# Lost Sheep Functional Requirements

## Global

- Javascript/HTML/Tauri application
- Native Tauri application

## Data Input

- Read a PDF in a specified format and convert it to JSON
- Import the JSON and convert it into database records
- Do NOT completely overwrite existing database, but flag potential differences
  (additions, deletions, address changes) with tools to replace,
  merge, add, or delete.

## Schema

- Names of Heads of Household (head, husband, wife, minor,
  grandparent, other). There may be multiple "heads" at one address
  (adult children, etc.)
- Names of minors
- Address
- Latitude, Longitude of address. Handle missing geocoords gracefully.
- Tags (unlimited number) linked to household. Can contain spaces.
- Tag management (CRUD)
- Reports by tag value
- Create groups that are close to each other geographically
- Records of visits or attempted visits, including comments
- Settings - theme, font size, etc.

## Operations

- Scale will typically be < 1000 active records. However, build so
  that up to 10,000 records will not be a problem.
- Read PDF into JSON. Notify user of errors on import.
- Read CSV (stub) into JSON
- Convert JSON to database
- Compare new data with existing and identify additions, deletions,
  potential merges, potential updates
- Manage update (additions, deletions, replace, updates, etc.) Bring
  in new data. Exact matches (normalized data match, white space does
  not count) are discarded from new data. Anything else is marked "for
  review". The review is user-driven. Comments can be added to each
  household (separate from visit comments). User can replace existing
  records with new, mark records as deleted (with comments) based on
  their absence in the update, but are saved in the deleted table.
- Tag household with one or more tags
- Group by tag
- Using current group, generate a list of geographically related
  households to visit using a specified household as a seed. Bound by
  number of households specified by user
- Update household records with visit/visit attempt. Includes date and
  comments. Details are included in comments.
- Report - output visits, including comments, within a specified date
  range.

## Security

- Use SQLCipher combined with the OS keychain
- Backup single SQLite file, prompted with encryption passphrase. The
  database is rekeyed using a passphrase-derived key (via Argon2id)
- Restore requires the encryption passphrase

## UI/UX

- Left sidebar with logo, hamburger menu icon, theme selection, font
  size selection, buttons for different operations
- Separate view for each primary operation (import, review/update,
  generate visit list
- Use Leaflet to display map with current group geocoded household
  locations overlayed. Be able to select households for inclusion in a
  "visit group" or select just one and generate nearby households to
  include in the group. Offline access is not needed, but if no
  internet access is available, display cached data for map.
- The user can specify a region (using the map) to cache. Perhaps a
  polygon definition and everything inside or touching the polygon is
  included in the cache. The cache can be deleted by the user. Have a
  setting that specifies whether to use a cache or not.
- No inline CSS, multiple themes (files only deliver colors, etc.)
  Each high-level view/operation has its own CSS file.
- App updates will be able to read existing database file so there
  should be no issues with installing a new version. App updates will
  be a new installer. These will be extremely rare. Targets are
  Windows, MacOS, and Linux, including deb and rpm based OS.
- Important operations should be logged. Multiple levels of logging
  available in the interface (error, warning, info, debug. Log viewer
  is available. I can provide an example of a log viewer.
- Settings include how long to retain deleted records, how many days
  before log files are deleted.
- There will be a filter/search mechanism (I can provide an
  example). Search is a simple text search across the entire record,
  including tags and comments. Filters include tags (others may
  follow). Search is always an implicit AND between keywords/filters.
- The complete results set from search/filters can be tagged with a
  new or existing tag.
- There will be an online help system that will cover common issues,
  including what to do if the OS keychain does not function properly
  (restore from backup) and how to do start for the first time.
- Keyboard navigation is not required. Themes must be color-blind
  friendly.
- Multi-head households are maintained as separate records since that
  is how they will be delivered in the import PDF. When grouped for a
  visit, all records that share the same address are included.
- Backup/Restore can browse to arbtrary file location within the
  user's home directory.
- Restore will read in the specified file and display a "before and
  after" of the changes that will occur if restored. I can provide an
  example of a Backup/Restore capability that we want to mimic.


I can provide examples of the side-bar based UI/UX, as well as CSS
theme files, from another app. I can also provide examples of a tag
implementation.
