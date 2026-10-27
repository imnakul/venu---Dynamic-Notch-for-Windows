# Venu Privacy Policy
Last updated: 27 October 2026

Venu is developed by Nakul Srivastava and published by JagatBandhu. This policy describes Venu 0.6.0 and its optional integrations.

## Local processing
Venu processes desktop features on your device. Depending on the features you use, this can include media titles, artists and artwork, system statistics, calendar event details, reminders and status items, selected images, Shelf file paths, clipboard text, Windows notification content, developer-tool usage data, and messages sent to its local notification endpoint. Glass rendering reads desktop pixels behind the notch into memory. No Venu account is required.

## Local configuration
Preferences and content you enter, including reminder text, priorities, image paths, Shelf paths and folder settings, and calendar feed URLs, are saved in a local configuration file, normally %APPDATA%\venu\config.json. This file is not encrypted. Treat private calendar links as secrets. Venu may import an older configuration from %APPDATA%\movingtext\config.json. A local startup log at %LOCALAPPDATA%\Venu\startup.log records limited lifecycle and diagnostic information.

## Shelf
Shelf stores paths rather than copies of your files. It reads configured folders to show recent files and can generate local thumbnails in memory. Selecting a pinned item can open the file or folder, launch an application, or run a script using Windows. Only pin and open items you trust.

## Clipboard history
Clipboard collection is off by default. If enabled, Venu reads Unicode text from the Windows clipboard and keeps its history in memory, with text limited to 4,000 characters per entry. It honors supported private-clipboard markers and the Windows flag that excludes content from clipboard history, but cannot guarantee that every sensitive item is marked by its source. Avoid copying sensitive text while history is enabled. Disabling clipboard history clears its in-memory entries. Clipboard history is not saved as a permanent archive.

## Windows notifications
In the packaged Store version, Windows notification collection is optional and off by default. After you connect it and grant Windows permission, Venu can read current Windows toast notifications, including the source app, title, body, and time, and display them in the notch. Retained records are in memory only and are not a permanent or restart-persistent archive. Removing a notification from Windows does not necessarily remove the retained Venu record immediately. Disconnecting stops collection but does not itself clear already retained notifications; use Venu dismissal or clear controls, or exit Venu, to remove them from memory. You can revoke access through Windows settings.

## Other in-memory data
Media details, parsed calendar events, the latest coding-tool usage snapshot, image and rendering caches, and Venu notifications are held in memory for the application session. Original images, files, and calendars remain under your control.

## Optional network connections
If you configure an online iCal feed, Venu downloads it directly from the URL you provide. The feed provider receives the request and ordinary connection information such as your IP address. Use a trusted HTTPS feed; a local ICS file can be used instead. Third-party calendar and media services have their own privacy practices.

## Developer integrations
Optional scripts send selected coding-tool usage metrics or event messages to a local HTTP endpoint, normally 127.0.0.1:18923. Some notification hooks can include an excerpt from tool output. Review the scripts and avoid sending sensitive content. The endpoint is bound to this computer and is not authenticated; other local software can submit events.

## Your choices and deletion
Disable optional collection and integrations in Settings, remove Shelf entries and calendar feeds, and clear Venu notifications using the in-app controls. To reset stored preferences, exit Venu and delete %APPDATA%\venu. If present, remove the legacy %APPDATA%\movingtext\config.json to prevent it being imported again. Delete %LOCALAPPDATA%\Venu\startup.log to remove the diagnostic log. Paths can differ because of Windows packaged-app storage behavior. Remove separately configured agent hooks to stop their submissions. Do not rely on uninstalling alone to remove all settings or other local data. These steps do not delete your original pinned files, images, scripts, or calendar files.

## Questions
Contact the developer through https://github.com/imnakul/venu---Dynamic-Notch-for-Windows/issues. Do not post private calendar links, clipboard contents, notification text, or other sensitive data in a public issue.

This policy will be updated if Venu introduces new data-handling features.
