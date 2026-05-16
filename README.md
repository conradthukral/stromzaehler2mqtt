# stromzaehler2mqtt

Hier entsteht ein Tool, das Daten über die optische Schnittstelle eines Stromzöhlers ausliest und über mqtt für HomeAssisstant bereitstellt.

Unterstützt werden initial:
* Stromzähler eBZ DD3 2R06 ETA-ODZ1
* Datenformat: EN62056-21 und EN62056-61

Beispieldaten sind im Verzeichnis example_data abgelegt.

Das Telegrammformat ist in [docs/telegram-format.md](docs/telegram-format.md) dokumentiert.

Das MQTT-Publishing und die Home-Assistant-Integration sind in [docs/mqtt.md](docs/mqtt.md) dokumentiert.

## Testen mit Sensor an remote-Maschine
* SSH-Tunnel aufbauen: `ssh -L 4000:localhost:4000`
* Auf remote-Maschine: `socat TCP-LISTEN:4000,reuseaddr,fork /dev/ttyUSB1,b9600,raw`
* lokal: `socat PTY,link=/tmp/ttyUSB0,raw TCP4:localhost:4000`

### Lokalen MQTT-Broker starten

Für lokale Tests steht ein Docker-Compose-Setup bereit, das einen Mosquitto-Broker ohne Authentifizierung startet und alle eingehenden Nachrichten auf der Konsole ausgibt:

```bash
docker compose -f local_testing/mqtt/docker-compose.yaml up
```

Die Konfiguration in `config.yaml` muss dann `host: localhost` und `port: 1883` verwenden.

### Home Assistant starten

Für Tests mit Home Assistant steht ein separates Docker-Compose-Setup bereit:

```bash
docker compose -f local_testing/homeassistant/docker-compose.yaml up
```

Home Assistant ist dann unter `http://localhost:8123` erreichbar. Nach dem ersten Start und der Accounterstellung die MQTT-Integration einrichten: **Settings → Devices & Services → Add Integration → MQTT**, Broker `host.docker.internal`, Port `1883`.
