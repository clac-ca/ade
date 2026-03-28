"""Load ADE config packages from discovered rule modules."""

from dataclasses import dataclass, field
from importlib import import_module
import pkgutil
from types import ModuleType

from ade_engine.config import EngineConfig, FieldRules, Rule


def _iter_modules(package_name: str) -> list[ModuleType]:
    package = import_module(package_name)
    module_infos = sorted(
        pkgutil.iter_modules(package.__path__, package.__name__ + "."),
        key=lambda module_info: module_info.name,
    )
    return [import_module(module_info.name) for module_info in module_infos]


@dataclass(frozen=True)
class _RegisteredRule:
    fn: Rule
    priority: int
    sequence: int


@dataclass
class _FieldRegistrations:
    detectors: list[_RegisteredRule] = field(default_factory=list)
    transforms: list[_RegisteredRule] = field(default_factory=list)
    validators: list[_RegisteredRule] = field(default_factory=list)


def _sorted_rules(registrations: list[_RegisteredRule]) -> list[Rule]:
    ordered = sorted(
        registrations,
        key=lambda registration: (registration.priority, registration.sequence),
    )
    return [registration.fn for registration in ordered]


@dataclass
class _Collector:
    fields: dict[str, _FieldRegistrations] = field(default_factory=dict)
    row_detectors: dict[str, list[_RegisteredRule]] = field(
        default_factory=lambda: {"header": [], "data": []},
    )
    hooks: dict[str, list[_RegisteredRule]] = field(default_factory=dict)
    _sequence: int = 0

    def _next_sequence(self) -> int:
        sequence = self._sequence
        self._sequence += 1
        return sequence

    def _registration(self, fn: Rule, priority: int) -> _RegisteredRule:
        return _RegisteredRule(
            fn=fn, priority=int(priority), sequence=self._next_sequence()
        )

    def _field_registrations(self, field_name: str) -> _FieldRegistrations:
        return self.fields.setdefault(field_name, _FieldRegistrations())

    def detector(self, field: str, fn: Rule, *, priority: int = 100) -> None:
        self._field_registrations(field).detectors.append(
            self._registration(fn, priority)
        )

    def transform(self, field: str, fn: Rule, *, priority: int = 100) -> None:
        self._field_registrations(field).transforms.append(
            self._registration(fn, priority)
        )

    def validator(self, field: str, fn: Rule, *, priority: int = 100) -> None:
        self._field_registrations(field).validators.append(
            self._registration(fn, priority)
        )

    def row_detector(self, kind: str, fn: Rule, *, priority: int = 100) -> None:
        if kind not in self.row_detectors:
            raise ValueError(f"Unsupported row detector kind: {kind}")
        self.row_detectors[kind].append(self._registration(fn, priority))

    def hook(self, event: str, fn: Rule, *, priority: int = 100) -> None:
        self.hooks.setdefault(event, []).append(self._registration(fn, priority))

    def build(self, *, name: str) -> EngineConfig:
        fields = {
            field_name: FieldRules(
                detectors=_sorted_rules(registrations.detectors),
                transforms=_sorted_rules(registrations.transforms),
                validators=_sorted_rules(registrations.validators),
            )
            for field_name, registrations in self.fields.items()
        }
        row_detectors = {
            kind: _sorted_rules(registrations)
            for kind, registrations in self.row_detectors.items()
        }
        hooks = {
            event: _sorted_rules(registrations)
            for event, registrations in self.hooks.items()
        }
        return EngineConfig(
            name=name,
            fields=fields,
            row_detectors=row_detectors,
            hooks=hooks,
        )


def _load_modules_into(config: _Collector, package_name: str) -> None:
    for module in _iter_modules(package_name):
        register = getattr(module, "register", None)
        if not callable(register):
            raise TypeError(f"Module '{module.__name__}' must define register(config)")
        register(config)


def load_engine_config(package_name: str, *, name: str) -> EngineConfig:
    config = _Collector()
    _load_modules_into(config, f"{package_name}.fields")
    _load_modules_into(config, f"{package_name}.row_detectors")
    _load_modules_into(config, f"{package_name}.hooks")
    return config.build(name=name)
