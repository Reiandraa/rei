import type { NavItem } from '../nav';
import { UNAVAILABLE_NOTE } from '../nav';
import { s } from '../strings';

/** Honest empty state: the area is planned, not available, and offers no fake actions. */
export function Placeholder({ item }: { item: NavItem }) {
  return (
    <section className="card" aria-label={`${item.label} availability`}>
      <p>{item.summary}</p>
      <p role="status" className="muted">
        <strong>{s.unavailableTitle}.</strong> {s.unavailableBody}
        {UNAVAILABLE_NOTE[item.id] ? ` ${UNAVAILABLE_NOTE[item.id]}` : ''}
      </p>
    </section>
  );
}
