---
name: what-is-on
use-it-when: You need to summarize the Indico events in the followed categories for a day or a week.
---
# What is on

Read the Indico categories `{input.indico_categories}` and build the summary
{person.name} asked for.

1. Fetch the events in each category with the Indico tools. Note for each
   event: title, start time (converted to the workspace's time zone), location
   or link, and the speaker.
2. Keep only events that start inside the period {person.name} asked about.
3. Format the list with the script that ships with this skill, which sorts
   the events and renders one compact line per event:

   ```sh
   python3 format-week.py events.json
   ```

   The script reads a JSON array of `{title, start, location, url}` objects
   from a file (or stdin) and prints the sorted, deduplicated week view.
4. Add one line per day only if that day has events. Link every event title
   to its Indico page.

If a category cannot be read, say which one and continue with the rest.
