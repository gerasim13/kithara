# kithara-assets

- Do not open independent disk AssetStore instances for the same root in one process. Share the store handle or its built indices; per-instance locks cannot serialize competing persisted index owners.

[Contract rationale](https://github.com/zvuk/kithara/wiki/kithara-assets).
