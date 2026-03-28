import re


def score_full_name(column) -> float:
    header = (column.header or "").strip().lower()
    if header in {"name", "full name", "employee name"}:
        return 1.0
    return 0.0


def normalize_full_name(value: object) -> str | None:
    if value is None:
        return None
    text = re.sub(r"\s+", " ", str(value).strip())
    return text or None


def register(config) -> None:
    config.detector("full_name", score_full_name)
    config.transform("full_name", normalize_full_name)
