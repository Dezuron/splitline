# Third-party notices

Splitline (the application formerly named `zapret-ui`) is an independent GUI manager. It is
**not** an official client of any project listed below and does not bundle their runtime in its
own release archive: the components below are downloaded at runtime from their upstream GitHub
releases and used unmodified.

## zapret-discord-youtube

- Repository: https://github.com/Flowseal/zapret-discord-youtube
- Author/maintainer: **Flowseal**
- License: MIT
- Copyright: © 2016–2026 bol-van; © 2024–2026 Flowseal

Splitline downloads the runtime from the upstream GitHub releases into a local folder and manages
it there. The runtime is not part of Splitline's own release archives.

## WinDivert

- Repository: https://github.com/basil00/WinDivert
- Author: **basil00**
- License: LGPL-3.0 or GPL-2.0 (at your choice)

WinDivert is distributed as part of the upstream zapret-discord-youtube runtime, not as part of
Splitline. The upstream runtime and WinDivert are downloaded unmodified from their respective
releases. Source code and license terms: https://github.com/basil00/WinDivert

## zapret (bol-van)

- Repository: https://github.com/bol-van/zapret
- License: MIT

The upstream Flowseal runtime contains binaries originating from bol-van/zapret.
Their copyright notice is retained above.

## Xray-core

- Repository: https://github.com/XTLS/Xray-core
- Author/maintainer: **XTLS** and contributors
- License: MPL-2.0

Splitline downloads the Windows build of Xray-core (`Xray-windows-64.zip`) from the upstream
GitHub releases and runs it as the VPN core. Xray-core is not included in Splitline's own release
archives. Source code and license terms: https://github.com/XTLS/Xray-core

## Geolocation data for `geosite:` bundles

- https://github.com/v2fly/domain-list-community — **v2fly** and contributors, MIT
- https://github.com/Loyalsoldier/v2ray-rules-dat — **Loyalsoldier**, GPL-3.0

Splitline downloads both datasets from their GitHub releases, merges them into a single
`geosite.dat` on the user's machine and keeps it in the local application folder. The merged file
is never published or redistributed by Splitline. Copyright and license terms belong to the
upstream projects.

## Flowseal/tg-ws-proxy (Telegram WS proxy)

- Repository: https://github.com/Flowseal/tg-ws-proxy
- Author/maintainer: **Flowseal** and contributors
- License: MIT

## Dezuron/tgws-cli (headless build of tg-ws-proxy)

- Repository: https://github.com/Dezuron/tgws-cli
- Author/maintainer: **Dezuron**
- License: MIT (headless build of Flowseal/tg-ws-proxy, MIT)

Splitline downloads `tgws-cli.exe` from this repository's GitHub releases and runs it as the local
Telegram MTProto-over-WebSocket proxy. The binary is not included in Splitline's own release
archives.
