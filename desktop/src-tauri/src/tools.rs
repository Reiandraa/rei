//! Runtime AI tool allow-list and run budget (docs/INTERFACES.md §4.1). The model's
//! output is untrusted: every call is parsed strictly (unknown tools and extra fields
//! rejected), bounded, and the only side effect a tool may have is producing a
//! Proposal through `proposals`. There is no write, shell, filesystem, network,
//! credential, settings or sync tool, so none can be invoked under prompt injection.
use serde::Deserialize;
use std::collections::HashSet;

pub const MAX_CALLS_PER_RUN: usize = 8;
pub const MAX_OUTPUT_BYTES: usize = 32 * 1024;
pub const MAX_EXCERPT_BYTES: usize = 8 * 1024;
pub const MAX_SEARCH_LIMIT: u32 = 20;
pub const MAX_AGENDA_DAYS: i64 = 31;
const MAX_ARG_JSON_BYTES: usize = 4096;

#[derive(Debug, PartialEq, Eq)]
pub enum ToolError {
    /// Unknown tool, malformed JSON or extra/missing fields.
    Schema(String),
    Bounds(&'static str),
    CallLimit,
    OutputLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchScope {
    Tasks,
    Events,
    Sources,
    Memory,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchArgs {
    pub query: String,
    pub scopes: Vec<SearchScope>,
    pub limit: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdArgs {
    pub id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RangeArgs {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposeArgs {
    #[serde(default)]
    pub target_id: Option<String>,
    pub fields: serde_json::Map<String, serde_json::Value>,
    pub rationale: String,
    #[serde(default)]
    pub source_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "name", content = "arguments", deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub enum ToolCall {
    SearchLocal(SearchArgs),
    GetTask(IdArgs),
    GetEvent(IdArgs),
    GetSourceExcerpt(IdArgs),
    ListAgenda(RangeArgs),
    ProposeTaskCreate(ProposeArgs),
    ProposeTaskUpdate(ProposeArgs),
    ProposeEventCreate(ProposeArgs),
    ProposeEventUpdate(ProposeArgs),
    ProposeTimeBlock(ProposeArgs),
    ProposeMemoryAdd(ProposeArgs),
}

#[derive(Debug, PartialEq, Eq)]
pub enum ToolKind {
    Read,
    Proposal,
}

impl ToolCall {
    pub fn kind(&self) -> ToolKind {
        match self {
            ToolCall::SearchLocal(_)
            | ToolCall::GetTask(_)
            | ToolCall::GetEvent(_)
            | ToolCall::GetSourceExcerpt(_)
            | ToolCall::ListAgenda(_) => ToolKind::Read,
            _ => ToolKind::Proposal,
        }
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Parses and bounds-checks one model-emitted tool call.
pub fn parse_call(raw: &str) -> Result<ToolCall, ToolError> {
    let call: ToolCall = serde_json::from_str(raw).map_err(|e| ToolError::Schema(e.to_string()))?;
    match &call {
        ToolCall::SearchLocal(a) => {
            if a.query.trim().is_empty() || a.query.chars().count() > 500 {
                return Err(ToolError::Bounds("query"));
            }
            if a.scopes.is_empty() || a.scopes.len() > 4 {
                return Err(ToolError::Bounds("scopes"));
            }
            if a.limit == 0 || a.limit > MAX_SEARCH_LIMIT {
                return Err(ToolError::Bounds("limit"));
            }
        }
        ToolCall::GetTask(a) | ToolCall::GetEvent(a) | ToolCall::GetSourceExcerpt(a) => {
            if !valid_id(&a.id) {
                return Err(ToolError::Bounds("id"));
            }
        }
        ToolCall::ListAgenda(a) => {
            let from: jiff::civil::Date = a.from.parse().map_err(|_| ToolError::Bounds("from"))?;
            let to: jiff::civil::Date = a.to.parse().map_err(|_| ToolError::Bounds("to"))?;
            let days = from
                .until(to)
                .map_err(|_| ToolError::Bounds("range"))?
                .get_days() as i64;
            if to < from || days > MAX_AGENDA_DAYS {
                return Err(ToolError::Bounds("range"));
            }
        }
        ToolCall::ProposeTaskCreate(a)
        | ToolCall::ProposeTaskUpdate(a)
        | ToolCall::ProposeEventCreate(a)
        | ToolCall::ProposeEventUpdate(a)
        | ToolCall::ProposeTimeBlock(a)
        | ToolCall::ProposeMemoryAdd(a) => {
            let needs_target = matches!(
                call,
                ToolCall::ProposeTaskUpdate(_) | ToolCall::ProposeEventUpdate(_)
            );
            match (&a.target_id, needs_target) {
                (Some(t), true) if valid_id(t) => {}
                (None, false) => {}
                _ => return Err(ToolError::Bounds("target_id")),
            }
            if a.rationale.chars().count() > 1000 {
                return Err(ToolError::Bounds("rationale"));
            }
            if a.source_ids.len() > 20 || !a.source_ids.iter().all(|s| valid_id(s)) {
                return Err(ToolError::Bounds("source_ids"));
            }
            let size = serde_json::to_vec(&a.fields)
                .map(|v| v.len())
                .unwrap_or(usize::MAX);
            if a.fields.is_empty() || size > MAX_ARG_JSON_BYTES {
                return Err(ToolError::Bounds("fields"));
            }
        }
    }
    Ok(call)
}

/// Per-run limits plus the set of source IDs the run actually saw, for citations.
#[derive(Default)]
pub struct RunBudget {
    calls: usize,
    output_bytes: usize,
    returned_ids: HashSet<String>,
}

impl RunBudget {
    /// Admits one call; the 9th call in a run is refused.
    pub fn admit(&mut self, raw: &str) -> Result<ToolCall, ToolError> {
        if self.calls >= MAX_CALLS_PER_RUN {
            return Err(ToolError::CallLimit);
        }
        self.calls += 1;
        parse_call(raw)
    }

    /// Accounts a tool result; refuses output that would exceed the run limit.
    pub fn record_output(&mut self, ids: &[&str], output: &str) -> Result<(), ToolError> {
        if self.output_bytes + output.len() > MAX_OUTPUT_BYTES {
            return Err(ToolError::OutputLimit);
        }
        self.output_bytes += output.len();
        self.returned_ids.extend(ids.iter().map(|s| s.to_string()));
        Ok(())
    }

    /// Citations to IDs not returned in this run are flagged `invalid_citation`.
    pub fn invalid_citations<'a>(&self, cited: &[&'a str]) -> Vec<&'a str> {
        cited
            .iter()
            .copied()
            .filter(|c| !self.returned_ids.contains(*c))
            .collect()
    }
}

/// Truncates at a UTF-8 boundary to at most `max` bytes.
pub fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}
