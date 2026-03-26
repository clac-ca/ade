import { useQuery } from "@tanstack/react-query";
import { getVersion } from "../api/system";

export function HomePage() {
  const versionQuery = useQuery({
    queryKey: ["system", "version"],
    queryFn: getVersion,
  });

  return (
    <section className="panel">
      <div className="hero">
        <p className="eyebrow">Parity-first foundation</p>
        <h2 className="hero__title">
          The frontend stays deliberately small and same-origin.
        </h2>
        <p className="hero__summary">
          Routing is handled by React Router, server data by TanStack Query, and
          API access by a thin client layer. Runtime health stays on explicit
          API endpoints, and operational metrics are exposed separately at
          `/metrics`.
        </p>
      </div>

      {versionQuery.isPending ? (
        <p className="status-note">
          Loading service metadata from `/api/version`…
        </p>
      ) : null}

      {versionQuery.isError ? (
        <p className="status-note status-note--error">
          Unable to load service metadata. {versionQuery.error.message}
        </p>
      ) : null}

      {versionQuery.data ? (
        <>
          <div className="status-grid">
            <section className="status-card">
              <p className="status-card__label">Service</p>
              <p className="status-card__value">{versionQuery.data.service}</p>
            </section>
            <section className="status-card">
              <p className="status-card__label">Version</p>
              <p className="status-card__value">{versionQuery.data.version}</p>
            </section>
          </div>

          <p className="status-note">
            This first slice keeps the client contract intentionally small: a
            minimal version endpoint for application identity and a standard
            Prometheus metrics endpoint for observability.
          </p>
        </>
      ) : null}
    </section>
  );
}
