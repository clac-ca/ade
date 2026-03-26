export function NotFoundPage() {
  return (
    <section className="panel not-found">
      <p className="eyebrow">404</p>
      <h2 className="not-found__title">This route does not exist.</h2>
      <p className="not-found__summary">
        The SPA fallback should handle valid frontend routes, but unknown paths
        still need a clear not-found boundary inside the client router.
      </p>
    </section>
  );
}
