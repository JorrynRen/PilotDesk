import React from "react";
import ReactDOM from "react-dom/client";
import { HashRouter } from "react-router-dom";
import App from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { I18nProvider } from "./components/common/I18nProvider";
import "./styles/globals.css";

import { applyThemeToDocument, THEMES, type Theme } from "./hooks/useTheme";

// Apply theme synchronously before React renders to prevent flash
(function applyThemeEarly() {
  try {
    const cached = localStorage.getItem('pilotdesk-theme') || 'system';
    const theme: Theme = THEMES.includes(cached as Theme) ? (cached as Theme) : 'system';
    applyThemeToDocument(theme, window.matchMedia('(prefers-color-scheme: dark)').matches);
  } catch { /* ignore */ }
})();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      <I18nProvider>
        <HashRouter>
          <App />
        </HashRouter>
      </I18nProvider>
    </ErrorBoundary>
  </React.StrictMode>,
);