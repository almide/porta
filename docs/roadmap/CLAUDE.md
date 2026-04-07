# Roadmap Rules

## Directory structure

```
docs/roadmap/
├── active/     Items currently being worked on
├── on-hold/    Items deferred to a later phase
├── done/       Completed items
├── CLAUDE.md   This file
├── README.md   Auto-generated — do not edit manually
└── generate-readme.sh
```

## File format

Every `.md` file in `active/`, `on-hold/`, and `done/` must follow this format:

For `active/` and `on-hold/`:

```markdown
<!-- description: Short English description (max 80 chars) -->
# Title in English

Content...
```

For `done/`:

```markdown
<!-- description: Short English description (max 80 chars) -->
<!-- done: YYYY-MM-DD -->
# Title in English

Content...
```

- **Line 1**: HTML comment with a concise description. Extracted by `generate-readme.sh` for the README table.
- **Line 2 (done/ only)**: Completion date. Used for sorting in the README.
- **Next line**: H1 title in English.
- **No status tags in titles**: Do not add `[ACTIVE]`, `[ON HOLD]`, or `[DONE]` to titles. The directory determines the status.

## Rules

1. **Language**: All titles and descriptions must be in English. Body content can be in any language.
2. **One item per file**: Each roadmap item gets its own `.md` file.
3. **Moving items**: Move files between `active/`, `on-hold/`, and `done/` to change status. Do not use tags.
4. **README is auto-generated**: Run `bash docs/roadmap/generate-readme.sh > docs/roadmap/README.md` to update.
5. **Description required**: Every file must have the `<!-- description: ... -->` comment on line 1. Without it, the README table will show an empty description.
