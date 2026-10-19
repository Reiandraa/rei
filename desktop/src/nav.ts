import { s } from './strings';

export interface NavItem {
  id: string;
  label: string;
  /** Daily pages, bottom utility navigation, or a retained internal route. */
  group: 'primary' | 'utility' | 'internal';
  /** Developer metadata (feature id and delivering phase); never shown in product screens. */
  feature: string;
  phase: string;
  /** Plain-English description, shown when the area is not available yet. */
  summary: string;
}

// Focused navigation. Deferred workspaces remain implemented in their modules, outside the sidebar.
export const NAV: NavItem[] = [
  { id: 'today', label: 'Today', group: 'primary', feature: 'P01', phase: 'planner', summary: 'Your agenda, tasks and time blocks for today.' },
  { id: 'tasks', label: 'Tasks', group: 'primary', feature: 'P01', phase: 'planner', summary: 'One-time tasks and repeating routines.' },
  { id: 'calendar', label: 'Calendar', group: 'primary', feature: 'P04', phase: 'planner', summary: 'Your month, with classes, events and deadlines.' },
  { id: 'inbox', label: 'Messages', group: 'primary', feature: 'P02/P03', phase: 'academic-inbox', summary: 'Your university email and original messages.' },
  { id: 'assistant', label: 'Assistant', group: 'primary', feature: 'P05', phase: 'ai-memory', summary: 'Important updates, summarized locally with their sources.' },
  { id: 'settings', label: 'Settings', group: 'utility', feature: 'P12', phase: 'foundation', summary: 'Connections, appearance, sync and advanced controls.' },
  { id: 'proposals', label: 'Review changes', group: 'internal', feature: 'P05', phase: 'planner', summary: 'Suggested changes waiting for your approval.' },
];

export const DEFAULT_SCREEN = 'today';
export const UNAVAILABLE_NOTE: Record<string, string> = { sync: s.unavailableSync };
