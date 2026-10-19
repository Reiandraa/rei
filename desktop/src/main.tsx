import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { AppErrorBoundary, Startup } from './Startup';
import './styles.css';

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <AppErrorBoundary><Startup><App /></Startup></AppErrorBoundary>
  </StrictMode>,
);
