"""The Gem object — the thing that round-trips between client and API.

Ported from BetterTroveTools `models/trove/gems.py`. The simulator generates a
Gem, the client holds the JSON, and posts it back to apply an action. Actions
are addressed by **stat position** (0/1/2) here (the API contract) rather than
by stat type as in the original UI. Computed fields (value, quality, power_rank,
…) are output-only and recomputed on every parse, so they round-trip safely.
"""

from __future__ import annotations

from datetime import UTC, datetime
from random import choice, randint, random, sample

from pydantic import BaseModel, ConfigDict, Field, computed_field

from .bases import (
    get_augment_base,
    get_empowered_gem_pr_threshold,
    get_gem_max_level,
    get_increment_power_rank_empowered,
    get_increment_power_rank_lesser,
    get_lesser_gem_pr_threshold,
    get_stat_base_empowered,
    get_stat_base_lesser,
    get_stat_threshold_empowered,
    get_stat_threshold_lesser,
)
from .constants import (
    GEM_ABILITIES,
    GEM_STAT_RESTRICTIONS,
    MAGIC_GEM_STAT_POOL,
    PHYSICAL_GEM_STAT_POOL,
    AugmentType,
    GemAbility,
    GemElement,
    GemRestriction,
    GemStatType,
    GemTier,
    GemType,
)


class Augment(BaseModel):
    type: AugmentType
    count: int = 0


class StatContainer(BaseModel):
    base: float = Field(default_factory=random)
    augments: list[Augment] = Field(default_factory=lambda: [Augment(type=t) for t in AugmentType])

    def add_augment(self, augment: AugmentType) -> None:
        for aug in self.augments:
            if aug.type == augment:
                aug.count += 1

    @computed_field
    @property
    def increase(self) -> float:
        return sum(get_augment_base(aug.type) / 100 * aug.count for aug in self.augments)

    @computed_field
    @property
    def value(self) -> float:
        return min(self.base + self.increase, 1)

    @computed_field
    @property
    def real_value(self) -> float:
        return self.base + self.increase


class Stat(BaseModel):
    type: GemStatType
    containers: list[StatContainer] = Field(default_factory=list)
    locked: bool = False

    @computed_field
    @property
    def augmentation_progress(self) -> float:
        if not self.containers:
            return 0.0
        return min(sum(c.real_value for c in self.containers) / len(self.containers), 1)

    def add_augment(self, augment: AugmentType) -> bool:
        if self.augmentation_progress == 1:
            return False
        for container in self.containers:
            if container.real_value >= 1:
                continue
            container.add_augment(augment)
            return True
        return False


class Gem(BaseModel):
    model_config = ConfigDict(use_enum_values=False)
    id: int = Field(default_factory=lambda: int(datetime.now(UTC).timestamp() * 100))
    tier: GemTier
    type: GemType
    element: GemElement
    restriction: GemRestriction | None = None
    ability: GemAbility | None = None
    level: int
    stats: list[Stat]
    augmentation: float | None = None

    @classmethod
    def create(cls, tier=None, type=None, element=None, restriction=None,
               augmentation=None, level=1, procs=None, generation=None) -> Gem:
        tier = choice(list(GemTier)) if not tier else GemTier(tier)
        type = choice(list(GemType)) if not type else GemType(type)
        element = choice(list(GemElement)) if not element else GemElement(element)
        aug = {"base": augmentation} if augmentation is not None else {}

        if type == GemType.LESSER:
            restriction = (
                choice(list(GemRestriction)) if restriction is None else GemRestriction(restriction)
            )
        else:
            restriction = None

        extra_containers = min(level, 15) // 5
        if not generation:
            if restriction is None:
                gem_stat_pool = choice([PHYSICAL_GEM_STAT_POOL, MAGIC_GEM_STAT_POOL])
            else:
                gem_stat_pool = (
                    PHYSICAL_GEM_STAT_POOL if restriction == GemRestriction.FIERCE else MAGIC_GEM_STAT_POOL
                )
            stat_types = sample(gem_stat_pool[element], 3)
            stats = [Stat(type=t) for t in stat_types]
            if element == GemElement.COSMIC:
                index = randint(0, 2)
                stats[index].type = GemStatType.LIGHT
                stats[index].locked = True
        else:
            stats = [Stat(type=GemStatType(t)) for t in generation]
            for stat in stats:
                if stat.type == GemStatType.LIGHT:
                    stat.locked = True

        for stat in stats:
            stat.containers.append(StatContainer(**aug))
        if procs is None:
            for _ in range(extra_containers):
                stats[randint(0, 2)].containers.append(StatContainer(**aug))
        else:
            for i, proc in enumerate(procs):
                for _ in range(proc):
                    stats[i].containers.append(StatContainer(**aug))

        max_level = get_gem_max_level(tier, type)
        level = min(level, max_level)
        ability = choice(list(GEM_ABILITIES[element])) if type == GemType.EMPOWERED else None
        return cls(tier=tier, type=type, element=element, restriction=restriction,
                   level=level, stats=stats, augmentation=augmentation, ability=ability)

    @property
    def _augment_level(self) -> dict:
        return {"base": self.augmentation} if self.augmentation is not None else {}

    def _check_position(self, position: int) -> None:
        if not 0 <= position < len(self.stats):
            raise IndexError(f"stat position {position} out of range (0..{len(self.stats) - 1})")

    # --- Actions (addressed by stat position) ------------------------------

    def augment_stat(self, position: int, augment_type: AugmentType | int) -> bool:
        """Apply one focus augment to the stat at `position`. False if already maxed."""
        self._check_position(position)
        return self.stats[position].add_augment(AugmentType(augment_type))

    def spark_stat(self, position: int) -> bool:
        """Reroll the stat type at `position` to another valid, unused type (False if locked)."""
        self._check_position(position)
        stat = self.stats[position]
        if stat.locked:
            return False
        in_use = [s.type for s in self.stats]
        if self.restriction:
            pool = (
                PHYSICAL_GEM_STAT_POOL if self.restriction == GemRestriction.FIERCE else MAGIC_GEM_STAT_POOL
            )
        else:
            pool = GEM_STAT_RESTRICTIONS
        unused = [s for s in pool[self.element] if s not in in_use]
        if GemStatType.PHYSICAL_DAMAGE in in_use and GemStatType.MAGIC_DAMAGE in unused:
            unused.remove(GemStatType.MAGIC_DAMAGE)
        if GemStatType.MAGIC_DAMAGE in in_use and GemStatType.PHYSICAL_DAMAGE in unused:
            unused.remove(GemStatType.PHYSICAL_DAMAGE)
        if not unused:
            return False
        stat.type = choice(unused)
        return True

    def flare_stat(self, position: int) -> bool:
        """Move one extra container off the stat at `position` to a random other stat."""
        self._check_position(position)
        stat = self.stats[position]
        if len(stat.containers) <= 1:
            return False
        moved = stat.containers.pop()
        other = choice([s for i, s in enumerate(self.stats) if i != position])
        other.containers.append(moved)
        return True

    def level_up(self) -> bool:
        max_level = get_gem_max_level(self.tier, self.type)
        if self.level >= max_level:
            return False
        self.level += 1
        if self.level in (5, 10, 15):
            self.stats[randint(0, 2)].containers.append(StatContainer(**self._augment_level))
        return True

    @property
    def container_count(self) -> int:
        return sum(len(stat.containers) for stat in self.stats)

    def set_level(self, level: int) -> bool:
        if self.level == level:
            return False
        max_level = get_gem_max_level(self.tier, self.type)
        self.level = min(level, max_level)
        final_containers = 3 + sum(1 for milestone in (5, 10, 15) if self.level >= milestone)
        diff = final_containers - self.container_count
        if diff > 0:
            for _ in range(diff):
                choice(self.stats).containers.append(StatContainer(**self._augment_level))
        elif diff < 0:
            for _ in range(-diff):
                stat = choice([s for s in self.stats if len(s.containers) > 1])
                if stat.containers:
                    stat.containers.pop()
        return True

    # --- Computed (output-only) --------------------------------------------

    @computed_field
    @property
    def ability_name(self) -> str | None:
        return self.ability.display_name if self.ability else None

    @computed_field
    @property
    def gem_name(self) -> str | None:
        if self.type == GemType.LESSER:
            if self.restriction is None:
                return f"{self.tier.display_name} Gem"
            return f"{self.restriction.display_name} {self.tier.display_name} Gem"
        return self.ability_name

    @computed_field
    @property
    def is_max_level(self) -> bool:
        return self.level == get_gem_max_level(self.tier, self.type)

    @computed_field
    @property
    def quality(self) -> float:
        values = [c.value for stat in self.stats for c in stat.containers]
        if not values:
            return 0.0
        return round((sum(values) / len(values)) * 100, 1) / 100

    @computed_field
    @property
    def power_rank(self) -> int:
        power_rank = 100 if self.type == GemType.EMPOWERED else 0
        if self.type == GemType.LESSER:
            thresholds = get_lesser_gem_pr_threshold(self.tier, self.element)
        else:
            thresholds = get_empowered_gem_pr_threshold(self.tier, self.element)
        for stat in self.stats:
            progress = thresholds[0] + (thresholds[1] - thresholds[0]) * stat.augmentation_progress
            power_rank += progress * len(stat.containers)
        increment_func = (
            get_increment_power_rank_lesser if self.type == GemType.LESSER
            else get_increment_power_rank_empowered
        )
        for _ in self.stats:
            for level in range(1, self.level + 1):
                power_rank += increment_func(self.tier, level)
        return round(power_rank)

    @computed_field
    @property
    def stat_values(self) -> list[dict]:
        increment_func = (
            get_increment_power_rank_lesser if self.type == GemType.LESSER
            else get_increment_power_rank_empowered
        )
        pr_increments = sum(increment_func(self.tier, level) for level in range(1, self.level + 1))
        base_func = get_stat_base_lesser if self.type == GemType.LESSER else get_stat_base_empowered
        threshold_func = (
            get_stat_threshold_lesser if self.type == GemType.LESSER else get_stat_threshold_empowered
        )
        calculated = []
        for stat in self.stats:
            stat_base = base_func(self.tier, self.element, stat.type)
            thresholds = threshold_func(self.tier, self.element, stat.type)
            progress = thresholds[0] + (thresholds[1] - thresholds[0]) * stat.augmentation_progress
            value = stat_base * progress * len(stat.containers) + stat_base * pr_increments
            calculated.append({stat.type.display_name: value})
        return calculated


def gem_lookups() -> dict:
    """Valid field values for gems: tiers, types, elements, stats, augments, abilities."""
    return {
        "tiers": [
            {"id": t.value, "name": t.display_name, "max_level": get_gem_max_level(t, GemType.LESSER)}
            for t in GemTier
        ],
        "types": [{"id": t.value, "name": t.display_name} for t in GemType],
        "elements": [{"id": e.value, "name": e.display_name} for e in GemElement],
        "restrictions": [{"id": r.value, "name": r.display_name} for r in GemRestriction],
        "stat_types": [{"id": s.value, "name": s.display_name} for s in GemStatType],
        "augment_types": [
            {"id": a.value, "name": a.display_name, "increase_percent": get_augment_base(a)}
            for a in AugmentType
        ],
        "abilities": [{"id": a.value, "name": a.display_name} for a in GemAbility],
        "abilities_by_element": {e.display_name: [ab.value for ab in GEM_ABILITIES[e]] for e in GemElement},
    }


class PartialGem(BaseModel):
    """A gem known only by tier/type/level/PR — used to reverse-check a PR value."""

    tier: GemTier
    type: GemType
    level: int
    power_rank: int

    @computed_field
    @property
    def expected_power_rank_range(self) -> tuple[int, int]:
        container_count = min(self.level, 15) // 5 + 3
        if self.type == GemType.LESSER:
            thresholds = get_lesser_gem_pr_threshold(self.tier, GemElement.WATER)
            increment_func = get_increment_power_rank_lesser
        else:
            thresholds = get_empowered_gem_pr_threshold(self.tier, GemElement.WATER)
            increment_func = get_increment_power_rank_empowered
        min_pr = thresholds[0] * container_count
        max_pr = thresholds[1] * container_count
        if self.type == GemType.EMPOWERED:
            min_pr += 100
            max_pr += 100
        for level in range(1, self.level + 1):
            pr_increment = increment_func(self.tier, level)
            min_pr += pr_increment * 3
            max_pr += pr_increment * 3
        return (round(min_pr), round(max_pr))

    @computed_field
    @property
    def is_within_expected_range(self) -> bool:
        min_pr, max_pr = self.expected_power_rank_range
        return min_pr <= self.power_rank <= max_pr

    @computed_field
    @property
    def progress(self) -> float:
        min_pr, max_pr = self.expected_power_rank_range
        if self.power_rank < min_pr:
            return -1.0
        if self.power_rank > max_pr:
            return 1.0
        return (self.power_rank - min_pr) / (max_pr - min_pr)
