# Videos

On Windows and Linux, AuroDownloader downloads the video of a web page, through two free
programs: **yt-dlp** (which reads the page) and **ffmpeg** (which joins picture and sound). It
does not download protected videos (DRM).

## Installing the tools

The first time, the "Add a video" box ("Video" button) offers "Install yt-dlp and ffmpeg":
they are downloaded from their official releases, their checksum or signature is checked, and
they take about 400 MB. Nothing is installed without that click.

yt-dlp updates itself, at most once a day (Options, Video, "Let yt-dlp update itself"), since
sites change often.

## Downloading a video

1. "Video", paste the address into "Address of the video page", then "Analyse". Sites
   without DRM work: PeerTube, Wikimedia Commons, Internet Archive… The page is read by
   yt-dlp.
2. AuroDownloader shows the title, the duration, the site and the available qualities, with
   their estimated size. Choose the **Quality**, or open "All formats" for a precise choice:
   each format with its Content (picture, sound, or both), its Picture (resolution), its Size
   and its Protocol. Choose the **Container** too ("Automatic" by default).
3. **Subtitles**: tick the languages you want; they are kept next to the video, in the site's
   format. A missing language is reported at the end.
4. "Save to" and, if needed, a "Subfolder", created in the chosen folder.
5. "Start" or "Download later".

The video goes through the same queues, speed limits and time slots as other downloads, and
resumes the same way after an interruption.

From the browser, the [extension](09-extension.md) does the same in one click.
