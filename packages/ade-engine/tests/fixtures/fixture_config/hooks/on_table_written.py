def log_table_written(ctx: object) -> None:
    ctx.logger.info("table written")


def append_summary(ctx: object) -> None:
    ctx.logger.info(f"summary appended for {ctx.sheet_name}")


def register(config) -> None:
    config.hook("on_table_written", log_table_written, priority=200)
    config.hook("on_table_written", append_summary, priority=300)
