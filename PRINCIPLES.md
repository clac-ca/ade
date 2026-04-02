# Principles

Distilled from Dave Farley's teaching on Continuous Delivery and Modern Software Engineering.

- Build quality into the work instead of relying on inspection to find problems later.
- Work in small batches because small changes are easier to understand, test, deploy, and recover.
- Keep the software in a releasable state and treat a broken commit stage as a stop-the-line problem.
- Use a deployment pipeline to produce evidence that each change is safe to release.
- Build the release candidate once and promote the same artifact through every stage.
- Deploy to every environment with the same automated mechanism and smoke test every deployment.
- Treat acceptance tests as executable specifications of system behaviour.
- Write acceptance tests in the language of the problem domain and focus on what the system does, not how it does it.
- Design for testability and deployability because they are first-class properties of good software.
- Name and structure the system around domain and runtime concepts, not transport, packaging, or emulator details.
- Keep reusable code in `packages/`, and keep app-owned runtime assets with the app that ships them.
- Make quality everybody's responsibility and ensure developers own the tests.
