import { Outlet } from "react-router-dom";

export function AppLayout() {
  return (
    <div className="app-shell">
      <header className="app-shell__header">
        <div className="app-shell__brand">
          <span className="app-shell__badge">ADE</span>
          <h1 className="app-shell__title">Automatic Data Extractor</h1>
        </div>
        <p className="app-shell__summary">
          A plain React SPA talking to the backend over HTTP, with routing and
          server state kept explicit.
        </p>
      </header>

      <main className="app-shell__main">
        <Outlet />
      </main>
    </div>
  );
}
