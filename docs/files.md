# Remote files

Open **Files** (⌘6) to browse a server through SFTP using its saved SSH authentication and host-key checks. The server must provide an SFTP subsystem and allow your account to access the requested paths.

## Browse and preview

Enter a remote path or expand folders in the tree. **Home** opens the account's initial SFTP directory. Filter by name, show hidden files, or move through pages of 100 entries. Listings are limited to 20,000 entries; open a smaller folder if a listing exceeds that limit.

Select a file to preview it:

| Format | Preview limit |
| --- | --- |
| UTF-8 text and code | 1 MiB |
| PNG, JPEG, GIF and WebP | 16 MiB |
| PDF | 16 MiB |

Content determines the preview type. Text is displayed as text, and PDFs render locally with page navigation and selectable text. Encrypted PDFs, unsupported formats and oversized files can be downloaded instead. Preview content is not saved to disk by Porthop.

## Upload and download

**Upload** selects one local file and sends it to the current remote directory. Existing remote files are not overwritten. Rename your local file if the destination name is already in use. Uploads do not preserve executable permissions.

**Download** opens the Mac save dialog, including a confirmation before replacing a local file. Porthop writes to a temporary file and replaces the destination only after a successful transfer. Failed or cancelled downloads leave the existing destination intact.

Transfers show progress and support cancellation. Leaving Files, switching servers, changing connection credentials or quitting cancels the associated work.

## Interrupted transfers

Uploads use a temporary `.porthop-upload-<uuid>` file in the remote directory. Porthop attempts to remove it after cancellation or failure, but connection loss or abrupt termination can leave it behind. Remove leftover staging files only after confirming that no upload is using them.

Permission errors appear in the workspace. At most eight file operations run concurrently. Listing and preview operations have a 60-second overall deadline after connection setup; individual SFTP requests also have response timeouts.
