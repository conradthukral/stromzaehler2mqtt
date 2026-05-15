# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

`stromzaehler2mqtt` reads data from an electricity meter (Stromzähler) via its optical interface (IEC 62056 D0 port) and publishes the readings to an MQTT broker, making them available to Home Assistant.

**Initially supported hardware:** eBZ DD3 2R06 ETA-ODZ1  
**Data formats:** EN62056-21 (mode D) and EN62056-61 (OBIS-based)

## Domain Context

- The optical interface on German smart meters uses infrared (IR) read/write head, typically connected as a serial port (e.g. `/dev/ttyUSB0`)
- EN62056-21 defines the data exchange protocol; EN62056-61 defines the OBIS code structure for identifying measurement values
- OBIS codes identify meter readings (e.g. `1-0:1.8.0` = total active energy import in Wh)
- Home Assistant auto-discovery works by publishing device config to `homeassistant/<component>/<device_id>/config` before publishing state values
