# What leaves this PC

Airlock is a local delay. The checks below run on the machine that is streaming.

- Word lists, spoken checks, and on-screen text recognition stay on this PC.
- Private terms stay on this PC. The desktop app seals them. The key is in the operating-system keychain. The browser preview has no keychain, so it keeps terms in that browser's profile.
- Speech recognition stays on this PC unless "Audio may be sent to Deepgram for speech recognition" is checked, a Pro license is saved, and a Deepgram key is in the OS keychain. That audio is sent to Deepgram. The key is not written into settings or logs.
- The local judge talks to Ollama on this machine. That transcript is not sent to a cloud model.
- The cloud judge sends the transcript to Anthropic only when "Transcript text may leave this PC" is checked and a key is saved. Private terms are not part of that request. The key is stored in the operating-system keychain.
- A stream key, once a broadcast exists, stays in the operating-system keychain. It is not written into settings or logs.
- A moderator connection, when it is turned on, sends the category, the time, and masked evidence to a computer that has the password. Private terms, the transcript, and stream keys are not included. The password stays in the operating-system keychain.
- Crash and latency reports are off unless the streamer opts in. A report is only a kind and a latency number. It does not include the transcript, private terms, or keys. No destination is configured, so opting in still sends nothing.

Shipped platform rule packs are text files in the app. They are not downloaded at stream time.
