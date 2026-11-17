
Current Obsidian setup works but is a bit scuffed, with a feeling of "duct-taped together".

If a custom editor becomes necessary:
Make it function like Obsidian, where it is an inline Markdown editor, with a file structure sidebar on the left.
Incorporate git, where diffs are shown with date and time, and who (user or AI agent)
Inline Python with runner where stdout is inserted into the note right below the code block, example:
```py
name = "Foo"
print("Hello, " + name + "!")
```
-> Hello, Foo!

Cache the entire file structure, and update it when any changes happen.

If an AI agent tries to read a non-text file, try to produce an AI-friendly version for it:
Excel -> CSV
Word -> md
PNG -> Visual intelligence description (md). This means that a visual AI model describes the image once, and it is cached for future use.
Unsure if cached files should be in the current directory or in a separate cache directory
Update the cache when the original changes
Keep a manual regenerate button for the cache

Fetched data from APIs
A md file contains an API endpoint, a response schema, and a format for how to structure the output into the visible portion of the md file. The user then clicks in the interface to register the endpoint and it will call automatically.
It will also describe when to call the endpoint, for example:
- In intervals (every 1 hour)
- At specific times (08:30 every day) can also specify which days, which months, etc.
And describe when the endpoint is unregistered.
Somehow allow API endpoints for Daily notes to be auto-created every day and register themselves. Perhaps a /Daily/API where the API md files are stored as templates that copy into the current Day's folder. The user would register those APIs inside /Daily/API instead of each day they exist in.
Note: /Daily/API is just a working name. If a better name exists, use that.
# In-file API
- Endpoint (https://foo.bar/endpoint)
- Headers (like Auth)
- Response schema
  { "foo": \["bar", "baz"\] }
- Data layout
  This is {foo\[1\]} -> This is baz

# Filesystem
- /Daily/YYYY/MM/DD-Mon/ 
	Folder of notes dedicated to a day
- / 
	Root folder, where unsorted notes go

# Daily
- Summary.md
	An AI-generated summary of all the notes in the folder

# Views

## Home View
Show all folders, with a thumbnail determined by a "thumb.png" inside the folder.
Does not show Daily
## Notes View
Show a sidebar of all folders and notes, where folders are collapsible.
Sidebar does not show Daily
Main view is of a note

## Daily View
Just like Notes view, but only for the current day, with the AI Summary shown as a special button above the other notes.

## Calendar View
Standard calendar view of the week, Monday as the first day of the week.
Importable webcal/iCal, with pattern-matched coloring for events, example:
- Events containing name "foo" are colored Green
Events can be clicked to create a new daily note linked to it. The note can have a backlink, but it must be date-time and range, and not reference the actual event, as it can be unreliable.
Any Note edits (not just Daily note edits) are shown on the calendar as lines or small blocks on the week view. Could this info come from git?
Daily notes unrelated to any events are shown above or below the day. If necessary, a button that opens a dialog shows the notes.