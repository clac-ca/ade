def score_header(row) -> float:
    values = [
        str(value).strip().lower() for value in row.values if value not in (None, "")
    ]
    if "full name" in values or "email address" in values:
        return 1.0
    if "name" in values or "email" in values:
        return 0.8
    return 0.0


def register(config) -> None:
    config.row_detector("header", score_header)
