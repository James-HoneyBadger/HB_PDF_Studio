# Optional large PDF fixtures

The large real-world PDFs have been removed from the default checkout, not from Git history. Small fixtures and test code are retained. Before running any regression or benchmark that requires the removed files, run `sh scripts/restore-large-pdf-fixtures.sh` from a full clone containing commit `6618fedeb2cf8488dc635e2f95a3a2ae9a1eb5f6`. This recreates the original paths without staging or overwriting existing files. Shallow clones must fetch the required commit first. The files are ignored by Git to avoid accidental recommits. Generated regression reports are also ignored.

Do not merge this change until CI and fixture-dependent tests have been verified with the restore step. This is not a history purge; a future history rewrite would require a separately preserved test corpus and coordinated migration.
