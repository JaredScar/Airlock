# What leaves this PC

Airlock is a local delay. The checks below run on the machine that is streaming.

- Word lists, spoken checks, and on-screen text recognition stay on this PC.
- Private terms stay on this PC. The desktop app seals them. The key is in the operating-system keychain. The browser preview has no keychain, so it keeps terms in that browser's profile.
- The local judge talks to Ollama on this machine. That transcript is not sent to a cloud model.
- The cloud judge sends the transcript to Anthropic only when "Transcript text may leave this PC" is checked and a key is saved. Private terms are not part of that request. The key is stored in the operating-system keychain.
- A stream key, once a broadcast exists, stays in the operating-system keychain. It is not written into settings or logs.
- Airlock does not send crash reports or usage statistics.

Shipped platform rule packs are text files in the app. They are not downloaded at stream time.
