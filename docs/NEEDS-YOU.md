# What needs you

Written 2026-10-31. This is only the work that cannot be finished from the code. Plan item numbers match `PLAN.md`. Do not paste a stream key or an API key into this file or into chat. Save those in the Airlock app. They stay in the OS keychain.

## Before the next broadcast

- [x] Restart Airlock so it is running the latest build.
- [x] In Streamlabs Desktop, use one destination only: Settings → Stream → Stream to custom ingest → Custom Streaming Server. URL `rtmp://127.0.0.1:1935/live`. Stream key `airlock`. Leave the Twitch destination off in Streamlabs. That key is only the hop from Streamlabs to this PC.
- [x] In Airlock Settings → Platform, save the Twitch stream key from Twitch's stream settings. This is the platform stream key, not an API client secret. After Go live, the status should read "Forwarding to Twitch." once the 8 second hold has passed. Ingest should read Connected.
- [x] Paste this development Pro license into Settings → License: `pro.2ee0536332375def1366cd3be1e70e96fb796986c1cc44b88613562ed07b87a7`. Word lists, dump, and spoken personal info work without it. The context judge, on-screen scanning, and cloud speech need it.
- [ ] Speech stays on this PC with the Whisper model already in the app data folder. Cloud speech stays off unless you check "Audio may be sent to Deepgram for speech recognition" and save a Deepgram key in that same panel.
- [ ] The context judge uses Ollama at `127.0.0.1:11434` with model `qwen3:8b` unless you check "Transcript text may leave this PC" and save an Anthropic key. Start Ollama before Go live if you want the local judge.

## Sessions to run

- [ ] **1.2** With Airlock already on Go live, start streaming from Streamlabs, stop, then start again. Do not restart Airlock in between. Ingest should go Connected, then Disconnected, then Connected.
- [ ] **1.1** Stream into Airlock for 10 minutes. Watch the held picture and sound and note whether they drift apart. The media path is already the ffmpeg program on PATH.
- [ ] **1.6** Stream to a private Twitch channel for 30 minutes. Once during that half hour, drop the network for about 10 seconds, then restore it. The broadcast should come back without a new stream key. Watch for gaps that do not recover.
- [ ] **1.12** Stream to Twitch for 1 hour. Say the words already on the word list and confirm each one is bleeped. In Task Manager, Airlock's CPU should stay under 15% aside from Streamlabs.

App data for a saved copy of a session is `%APPDATA%\com.badger.airlock`.

## Decisions, whenever you want them

- [ ] Final product name. "Airlock" is a working name and still needs a trademark check.
- [ ] Pricing: one-time purchase or subscription, and what stays in the free tier.
- [ ] Should Dump also rebuild the delay, or only mute what is still in the hold? Today it mutes the hold and does not rebuild the delay.

## Bring these when you have them

These stay open until the thing named here exists. None of them block a normal broadcast.

- [ ] **2.2** Install the CUDA toolkit if you want Whisper on the GPU. Today it runs `small.en` on the CPU. The bar is a word finalized within 1.5 seconds on this RTX 5090.
- [ ] **2.6** A licensed slur list. `config/wordlists/slurs.txt` stays empty on purpose. Extra profanity and allowlist lines are already in Settings.
- [ ] **4.6, 5.3, 5.8** OBS, if you want scene names, automatic encoder setup, and "start when OBS launches." Streamlabs is already a first-class publisher. The tray Show and Quit menu is already in the app.
- [ ] **6.2 and 3.8** A recording of real speech over game noise. The eval clips already cover a swear, a phone number, and a threat. The 0.85 ToS-risk score waits on that recording.
- [ ] **8.1** A code-signing certificate and an update host. An unsigned Windows installer already exists.
- [ ] **8.2** A Mac, for the Mac build and Apple Vision text recognition.
