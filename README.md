# stromzaehler2mqtt

Hier entsteht ein Tool, das Daten über die optische Schnittstelle eines Stromzöhlers ausliest und über mqtt für HomeAssisstant bereitstellt.

Unterstützt werden initial:
* Stromzähler eBZ DD3 2R06 ETA-ODZ1
* Datenformat: EN62056-21 und EN62056-61

Beispieldaten sind im Verzeichnis example_data abgelegt.

Das Telegrammformat ist in [docs/telegram-format.md](docs/telegram-format.md) dokumentiert.

## Testen mit Sensor an remote-Maschine
* SSH-Tunnel aufbauen: `ssh -L 4000:localhost:4000`
* Auf remote-Maschine: `socat TCP-LISTEN:4000,reuseaddr,fork /dev/ttyUSB1,b9600,raw`
* lokal: `socat PTY,link=/tmp/ttyUSB0,raw TCP4:localhost:4000`
