# REI

A calmer university day: important email updates, proposed tasks, repeating
classes, and a daily planner in one local-first desktop app.

- [Product and features](https://reiandraa.com/projects/rei/)
- [Try the browser demo](https://reiandraa.com/projects/rei/demo/)
- [Desktop source and setup](desktop/README.md)
- [Windows and macOS beta downloads](https://github.com/Reiandraa/rei/releases)

## REI OS desktop

The current React/TypeScript and Rust/Tauri application lives in [`desktop/`](desktop/).
It includes the university inbox workflow, source-linked briefings, task approval,
repeating schedules, month calendar, appearance settings, and optional local
Ollama or user-supplied Claude/OpenAI APIs. The current university connector is HIZ.

This is a development preview. Teams access, cloud sync, a full conversational
assistant with searchable memory, and signed cross-platform distribution are
not offered as finished features. The browser demo uses fictional data and
prewritten summaries, without connecting accounts or running AI.

## Earlier Python prototype

The original `ai.py`, `config.py`, `main.py`, `requirements.txt`, and `test.py`
remain at the repository root. That earlier REI 0.1 experiment explored a
personal AI assistant with memory and tool use. It is separate from the
current desktop application; follow the desktop guide for REI OS.
