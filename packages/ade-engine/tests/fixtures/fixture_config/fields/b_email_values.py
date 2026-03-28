def score_email_values(column) -> float:
    if any("@" in str(value) for value in column.sample_values):
        return 0.7
    return 0.0


def require_at_symbol(value: object) -> str | None:
    if value is None:
        return None
    text = str(value).strip()
    return None if "@" in text else "Invalid email"


def register(config) -> None:
    config.detector("email", score_email_values, priority=300)
    config.validator("email", require_at_symbol, priority=400)
