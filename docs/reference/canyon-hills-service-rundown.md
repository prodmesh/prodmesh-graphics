> **Reference document, written by the owner 2026-08-16.** A real Sunday at
> Canyon Hills, minute by minute, as it runs *today* on ProPresenter. Kept
> verbatim.
>
> This is the most load-bearing requirements document in the repo. It is where
> "an Item has several destinations" stopped being a design argument and became
> an observation, and it is what `docs/reference/what-the-rundown-changed.md`
> was written against. Read that second.

# Example Cue Rundown for Canyon Hills Service  
  

| Relative to program start | Action | Destinations |
| ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------- |
| [beyond -1:30] | Auto-rotating slides showing various events and registration at the church. A timer element in the top right corner counts down to service start.  | 1080p Playback Out |
| -1:30 | Operator clicks Announcements. Timer element auto-clears (slides out) at zero. | Video: 1080p Playback Out, Audio: 4 channels, played into Dante Virtual Soundcard |
| 0:00 | Service Call to Worship, Operator triggers name tag for worship leader | Lower Third Key/Fill |
| 0:30 | Service Call to worship, Operator hides name tag, triggers Bible Verse for the worship | Lower Third Key/Fill; + Full Screen 1080p version of the verse |
| ~1:30 | Operator hides verse | All Blank |
| ~2:00 | Worship song 1 intro begins, operator clicks first song in playlist | Still blank (no lyrics yet) |
| 2:10 | Verse 1 begins, operator beings pressing Right arrow key to step thru song, following the music. In advanced services, this can be programmed to LTC or Midi Timecode.  | Lower Third Key/Fill + Full Screen 1080p version of the lyric. LED Wall also has its own look with the lyric. |
| ~6:00 | Second song intro begins, operator clicks second song in playlist | Blank |
| ~6:10 | Second Song Verse 1, operator again pressing right arrow key to follow through song | Lower Third Key/Fill + Full Screen 1080p version of the lyric. LED Wall also has its own look with the lyric. |
| ~10:41 | Second Song, end of planned lyrics. Worship leader suddenly decides to sing the Chorus again. Operator clicks the first slide in the “Chorus” to go back, and starts to arrow through again | Lower Third Key/Fill + Full Screen 1080p version of the lyric. LED Wall also has its own look with the lyric. |
| ~14:00-27:00 | Song 3/4 repeat as above | Lower Third Key/Fill + Full Screen 1080p version of the lyric. LED Wall also has its own look with the lyric. |
| 27:10 | Songs complete, lights in room dim and the operator triggers a Sermon Bumper video, a 15 second clip allowing us to clear stage and put out a podium | Video: 1080p Playback Out, Audio: 2 channels, played into Dante Virtual Soundcard |
| 27:25 | Video cleared by operator after the director takes a camera. Operator triggers Pastor’s lower third name tag. | Lower Third Key/Fill |
| 27:40+ | Lower third dismissed. | Now all blank |
| 28:00 | Pastor says to open bible to the chapter/verse of the day. Operator selects a lower third chyron-style graphic with the sermon title and bible book/chapter/verses | Lower Third Key/Fill + Full Screen 1080p version of the lyric. LED Wall also has its own look with the lyric. |
| 29:00 | Pastor begins reading the verse. The “Bible verse look” displays the first few lines of the verse (as much as it fits on the lower third and LED wall) | Lower Third Key/Fill + Full Screen 1080p version of the lyric. LED Wall also has its own look with the lyric. |
| 29:30 | We get to the next part of the verse. The previous section cleanly fades to the next section and the box animation tween expands/contracts to fit that text cleanly. (This tweening doesn’t actually happen today, ProPresenter doesn’t support it because it doesn’t support responsive design) | Lower Third Key/Fill + Full Screen 1080p version of the lyric. LED Wall also has its own look with the lyric. |
| 30:00-60:00 | Rest of sermon, with various lower thirds and full screen graphics. Lower thirds always have a full screen version we can take if we need to, and a LED wall version. Full screen graphics typically blank out the LED wall leaving only a background.  | Lower Third Key/Fill + Full Screen 1080p version of the lyric. LED Wall also has its own look with the lyric. |
| 60:00 | Service concludes, pastor mentions that prayer is available in front. Operator selects “Prayer is available after the service” slide | Lower Third Key/Fill + Full Screen 1080p version of the lyric. LED Wall also has its own look with the lyric. |
  
  
