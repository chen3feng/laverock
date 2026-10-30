# Phone remote

English | [中文](remote.zh-CN.md) · [← README](../README.md)

A phone on the same network can use the whole studio — synthesise, browse
history, manage voices — with no second frontend: it loads the same web app
the desktop shell shows. This page describes how pairing and the LAN
listener work.

## Turning it on

Open **Settings → Phone remote → LAN access**. The server then opens a second
listener on the machine's LAN address (the loopback listener stays separate),
and **Show pairing QR code** produces a QR code plus a six-digit code.

Scan the QR code with the phone's camera, or open the address shown next to
it and enter the code. The pairing is remembered by a cookie, so the phone
stays paired until you revoke it under **Settings → Phone remote → Paired
devices**.

The LAN address is resolved at startup. If the machine switches networks,
turn LAN access off and on again to pick up the new address.

## Security model

- **Loopback is trusted; the LAN listener is not.** Everything except `/`,
  `/api/session` and `/api/pair/claim` requires a device token, accepted
  either as `Authorization: Bearer …` or as the `laverock_device` cookie
  (needed for `<audio>` elements and streamed responses).
- Pairing codes are single-use, expire after five minutes, and are
  invalidated after eight wrong attempts. Only the loopback listener can
  create one.
- Tokens are 32 random bytes, stored only as SHA-256 hashes in
  `data/remote.json` (mode 0600). Revoking a phone takes effect on its next
  request.
- The pairing state and the LAN toggle survive restarts (`lan_enabled` is
  persisted in the settings file).

## HTTPS

The LAN listener serves **HTTPS** with a self-signed certificate kept in
`data/tls/` and regenerated when the LAN address changes. A secure context
is what lets the phone use the microphone for recording a reference clip.

On the first visit the browser warns that the certificate is untrusted;
choose **advanced → proceed**. The certificate's SHA-256 fingerprint is shown
next to the QR code, so a cautious user can verify it matches before
trusting the page.

Because the certificate is self-signed, a phone that previously paired with
a different LAN address will warn again after the certificate is
regenerated — revoke and pair it again if the browser refuses the cookie.

## Limits

- Only devices on the same local network can reach the listener; it is not
  exposed to the internet and there is no relay, tunnel or account.
- The desktop app must be running (or the server started) for the phone to
  connect; closing the desktop window only hides it.
