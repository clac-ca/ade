def score_email_header(column) -> float:
    header = (column.header or "").strip().lower()
    if header in {"email", "email address"}:
        return 1.0
    return 0.0


def score_email_values(column) -> float:
    if any("@" in str(value) for value in column.sample_values):
        return 0.7
    return 0.0


def strip_whitespace(value: object) -> str | None:
    if value is None:
        return None
    text = str(value).strip()
    return text or None


def lowercase(value: object) -> str | None:
    if value is None:
        return None
    text = str(value).strip().lower()
    return text or None


def require_at_symbol(value: object) -> str | None:
    if value is None:
        return None
    text = str(value).strip()
    return None if "@" in text else "Invalid email"


def register(config) -> None:
    config.detector("email", score_email_header, priority=200)
    config.detector("email", score_email_values, priority=300)
    config.transform("email", strip_whitespace, priority=200)
    config.transform("email", lowercase, priority=200)
    config.validator("email", require_at_symbol, priority=400)
