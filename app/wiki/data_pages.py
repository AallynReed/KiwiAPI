"""Wiki pages generated from game data, each with an editable write-up.

A write-up is stored at ``data/<name>`` with a fixed title and read at ``/<name>``,
so ``<name>`` is reserved for plain articles.
"""

DATA_PAGES: dict[str, str] = {
    "delve-modifiers": "Delve Modifiers",
}


def title_for(slug: str) -> str | None:
    """The fixed title of a ``data/<name>`` slug, else None."""
    prefix, _, name = slug.partition("/")
    return DATA_PAGES.get(name) if prefix == "data" else None
