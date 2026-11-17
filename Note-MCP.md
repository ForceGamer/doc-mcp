List notes, options:
- Dir: str (directory to list, or date to list for Daily)
- Tree: bool (show entire notebook tree)

Tools overview:
- List notes(path) must be a directory
- Read note(path) must be of type md
- Read all notes(path) mix of the first 2. Must be a directory. Only reads md files. Also ignores Summary.md

These should be ignored by all tools:
- Daily directory itself. Should only be accessed with a date.
- Hidden directories. Including directories starting with "." on Windows.
- Directories and files ending in "-s".

## Today's notes
Unique feature

Each day has a unique folder `/Daily/YYYY/MM/DD-DAY`
Example: `/Daily/2026/11/03-TUE`

## Schema

### Daily folder
- Path: /Daily/YYYY/MM/DD-DAY
- Summary.md: AI-written note that is a short summary of the day.
- Other md files: Notes relevant to the day.

### Cache
##### Today's folder
- Summary: whole summary in RAM
- All files: paths and filetype (md, xlsx, docx)
##### Notes
- All files and folders: paths and filetype
- Today's Daily notes