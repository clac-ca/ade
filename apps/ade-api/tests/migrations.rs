use ade_api::embedded_migrations;

#[test]
fn embedded_migrations_use_small_ordered_versions() {
    let runner = embedded_migrations::migrations::runner();
    let migrations = runner.get_migrations();

    assert_eq!(migrations.len(), 1);
    assert_eq!(migrations[0].version() as i32, 1);
    assert_eq!(migrations[0].name(), "initial_schema");
}
