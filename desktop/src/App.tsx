import { useEffect, useRef, useState } from 'react';
import { api } from './ipc';
import { DEFAULT_SCREEN, NAV, type NavItem } from './nav';
import { InboxScreen } from './screens/Inbox';
import { Placeholder } from './screens/Placeholder';
import { CalendarScreen, ProposalsScreen, TasksScreen, TodayScreen } from './screens/Planner';
import { SettingsScreen } from './screens/SettingsScreen';
import { s } from './strings';
import { applyTheme, applyAccent, applyTextSize, applyMotion } from './theme';
import { Icon } from './components/Icon';
import { AssistantScreen } from './screens/Assistant';

function screenFromHash(): string {
  const id = window.location.hash.replace(/^#\/?/, '').split('?')[0];
  if(id==='memory')return 'assistant';
  if(id==='sync')return 'settings';
  return NAV.some((n) => n.id === id) ? id : DEFAULT_SCREEN;
}

function NavList({ items, screen }: { items: NavItem[]; screen: string }) {
  return (
    <ul>
      {items.map((n) => (
        <li key={n.id}>
          <a
            href={`#/${n.id}`}
            aria-current={n.id === screen ? 'page' : undefined}
            className={n.id === screen ? 'active' : undefined}
          >
            <Icon name={n.id} />{n.label}
          </a>
        </li>
      ))}
    </ul>
  );
}

export function App() {
  const [screen, setScreen] = useState(screenFromHash);
  const heading = useRef<HTMLHeadingElement>(null);

  useEffect(() => {
    const onHash = () => setScreen(screenFromHash());
    window.addEventListener('hashchange', onHash);
    return () => window.removeEventListener('hashchange', onHash);
  }, []);

  useEffect(() => {
    heading.current?.focus();
  }, [screen]);

  // Restore saved appearance and reading preferences on every app start.
  useEffect(() => {
    api.settingsGet('text_size').then(s=>applyTextSize(s?.value),()=>applyTextSize('standard'));
    api.settingsGet('motion').then(s=>applyMotion(s?.value),()=>applyMotion('system'));
    api.settingsGet('accent').then((s)=>applyAccent(typeof s?.value === 'string'?s.value:'green'),()=>applyAccent('green'));
    api.settingsGet('theme').then((t) => applyTheme(typeof t?.value === 'string' ? t.value : 'system'), () => applyTheme('light'));
  }, []);

  const item = NAV.find((n) => n.id === screen) ?? NAV[0];

  return (
    <div className="shell">
      <nav className="sidebar" aria-label={s.navPrimary}>
        <div className="brand"><Icon name="sprout" /><span>REI</span></div>
        <div className="sidebar-links"><NavList items={NAV.filter((n) => n.group === 'primary')} screen={screen} />
        </div><div className="sidebar-utility"><NavList items={NAV.filter(n=>n.group==='utility')} screen={screen}/></div><div className="sidebar-foot"><span className="avatar"><Icon name="sprout" /></span><span>Personal space<small>On this computer</small></span></div>
      </nav>
      <div className="content-shell"><div className="topbar"><span>{item.id === 'today' ? 'Your day, in one place' : item.summary}</span><span className="local-badge">Local workspace</span></div><main className={`content ${item.id === 'today' ? 'content-today' : ''}`}>
        <h1 ref={heading} tabIndex={-1}>
          {item.label}
        </h1>
        {item.id === 'settings' ? <SettingsScreen />
          : item.id === 'today' ? <TodayScreen />
          : item.id === 'tasks' ? <TasksScreen />
          : item.id === 'assistant' ? <AssistantScreen />
          : item.id === 'calendar' ? <CalendarScreen />
          : item.id === 'inbox' ? <InboxScreen />
          : item.id === 'proposals' ? <ProposalsScreen />
          : <Placeholder item={item} />}
      </main><footer className="app-footer">Local-first · Account and model settings stay in Settings</footer></div>
    </div>
  );
}
