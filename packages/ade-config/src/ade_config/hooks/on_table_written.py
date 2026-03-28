def rename_output_sheet(ctx: object) -> None:
    ctx.worksheet.title = "Normalized Output"


def add_summary_sheet(ctx: object) -> None:
    summary = ctx.workbook.create_sheet("Summary")
    summary["A1"] = f"Summary for {ctx.worksheet.title}"


def register(config) -> None:
    config.hook("on_table_written", rename_output_sheet, priority=200)
    config.hook("on_table_written", add_summary_sheet, priority=300)
