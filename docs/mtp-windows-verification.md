# Windows MTP verification checklist

> **Status: not yet implemented.** This checklist is written ahead of
> the work so the verification gap is recorded rather than discovered
> late. As of Phase 1 there is no `WpdTransport`, no
> `src-tauri/src/device/transport/wpd.rs`, and no `windows-latest` CI
> job. Windows users reach a device today through `FsTransport` by
> pointing TuxTunes at a mounted path, which works but is not native
> MTP.

When the Windows Portable Devices backend lands (Phase 6) it will be
compiled, linted and unit-tested in CI on `windows-latest`, with the
whole sync engine running there against `FsTransport`. What CI **will
not** be able to prove is that its COM calls drive real hardware
correctly, because no Windows machine with a phone attached is
available to this project.

Until someone works through this checklist on real hardware, Windows
MTP should ship labelled **beta** in the device UI, with `FsTransport`
still selectable so a user can fall back to a mounted device.

## Before you start

- A Windows 10 or 11 machine.
- At least two devices, ideally: an Android phone (Pixel or Samsung —
  they differ) and an Android-based DAP (FiiO, HiBy, Shanling).
- Set each device's USB mode to **File transfer / MTP**, not charging.
- Build with `cargo build --release` and run TuxTunes from that build.

Record the device model and Android version beside each result. A
failure on one device and a pass on another is the most useful thing
this checklist can tell us.

## Checklist

| #   | Behaviour                | How to check                                                                                     | Pass criteria                                                                                                                                                                                 |
| --- | ------------------------ | ------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | **Enumeration**          | Attach the device, then use the sidebar's rescan button.                                         | The device appears by name within a few seconds. Unplugging and re-attaching reuses the same row, keeping its selection — it does not create a duplicate.                                     |
| 2   | **Stable device key**    | Note the row, unplug, replug into a _different_ USB port.                                        | Still one row, same selection, same manifest. If a duplicate appears, the serial is not being read and `key_is_weak` should be set.                                                           |
| 3   | **Free space**           | Open the device view and press Preview.                                                          | Free and total bytes are reported and roughly match what Windows Explorer shows for the device.                                                                                               |
| 4   | **Directory creation**   | Select a playlist whose tracks span several artists and albums, then Sync.                       | The full `Music/<Artist>/<Album>/` tree is created; no flattening, no missing levels.                                                                                                         |
| 5   | **Large-file write**     | Sync an album of 24-bit FLACs (at least one file over 100 MB).                                   | Every file arrives complete. Compare sizes against the library files; they must match exactly.                                                                                                |
| 6   | **Bit-exactness**        | Hash one transferred FLAC on the device and in the library.                                      | Identical hashes. This is the core promise of the feature.                                                                                                                                    |
| 7   | **Rename**               | Watch the device during a sync.                                                                  | Files appear at their final names. No `.tuxpart` files remain after the sync completes.                                                                                                       |
| 8   | **Delete**               | Deselect the playlist and sync again.                                                            | The tracks are removed from the device, and the now-empty album and artist directories go with them.                                                                                          |
| 9   | **Non-destruction**      | Put a file on the device by hand (Explorer), under `Music/Manual/`. Sync with mirror-deletes on. | The hand-placed file is untouched. This must never fail.                                                                                                                                      |
| 10  | **Playlist objects**     | Sync a playlist, then open the device's stock music app.                                         | The playlist appears in the stock app, and the `.m3u8` is present under `Music/Playlists/`. If only the `.m3u8` is there, a `playlist_object_failed` warning should explain why.              |
| 11  | **Playlists play**       | Open the playlist in Poweramp or USB Audio Player Pro.                                           | Every track resolves and plays. A track that fails to resolve means the relative paths are wrong for this device's mount point — record the mount path.                                       |
| 12  | **Mid-write unplug**     | Start syncing a large album, unplug the cable partway.                                           | TuxTunes reports a failure rather than hanging. Replug and sync again: the interrupted file is re-sent, no `.tuxpart` survives, and the manifest holds no row for the file that did not land. |
| 13  | **Out of space**         | Select more music than the device can hold.                                                      | Preview warns before the sync starts. If a sync is run anyway, it stops with a clear out-of-space error and everything already transferred stays intact and playable.                         |
| 14  | **Cancel**               | Start a large sync and press Cancel.                                                             | It stops within a second or two, not at the end of the file. Files already transferred remain, and are recorded.                                                                              |
| 15  | **Explorer coexistence** | While TuxTunes has the device open, browse it in Explorer.                                       | Both work. WPD is in-box and should not require exclusive access; if Explorer is locked out, the session handling needs revisiting.                                                           |

## Reporting

Open an issue titled `Windows MTP verification: <device model>` with the
table above filled in, the Android version, and the contents of the run
log (`%APPDATA%\tuxtunes\logs\device-*.log`) for any row that failed.
