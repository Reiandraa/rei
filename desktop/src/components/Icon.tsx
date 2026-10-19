export function Icon({ name }: { name: string }) {
  const paths: Record<string, string> = {
    today: 'M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8 M12 2v2 M12 20v2 M2 12h2 M20 12h2 M5 5l1.5 1.5 M17.5 17.5 19 19 M5 19l1.5-1.5 M17.5 6.5 19 5',
    tasks: 'M20 11v1a8 8 0 1 1-5-7 M8 11l4 4 9-10',
    inbox: 'M4 4h16v16H4z M4 13h5l1 3h4l1-3h5',
    settings: 'M4 6h16 M4 12h16 M4 18h16 M8 3v6 M16 9v6 M10 15v6',
    calendar: 'M4 5h16v16H4z M8 2v6 M16 2v6 M4 10h16',
    assistant: 'M12 3l2.5 6.5L21 12l-6.5 2.5L12 21l-2.5-6.5L3 12l6.5-2.5z',
    proposals: 'M5 7h14 M5 12h10 M5 17h7 M16 17l2 2 4-4',
    sprout: 'M12 22v-9 M12 16C4 16 3 11 3 6c6 0 9 3 9 10 M12 12c0-7 3-10 9-10 0 6-3 10-9 10',
  };
  return <svg className="rei-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d={paths[name] ?? 'M5 7h14 M5 12h14 M5 17h14'} /></svg>;
}
