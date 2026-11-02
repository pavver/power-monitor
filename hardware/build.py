"""Draws the schematic sheets as SVG files next to this script.

    python3 hardware/build.py

Sheet 1 (power and the battery divider) and sheet 3 (the 220 V sensor) are
the same for every build; sheet 2 has one version per board.
"""

from pathlib import Path

HERE = Path(__file__).parent

STYLE = """<style>
text{font-family:'DejaVu Sans Mono',Consolas,Menlo,monospace;font-size:13px;fill:#1d2326}
.ref{font-weight:600}
.pin{font-size:11px;fill:#5b666b}
.set{fill:#0a5fa8;font-weight:600}
.warn{fill:#b3261e;font-weight:600}
.w{stroke:#1d2326;stroke-width:1.6;fill:none}
.b{fill:#fff;stroke:#1d2326;stroke-width:1.6}
.dash{fill:none;stroke:#5b666b;stroke-width:1.2;stroke-dasharray:5 4}
.solid{fill:#1d2326;stroke:none}
.zone{fill:rgba(179,38,30,.07);stroke:#b3261e;stroke-width:1.2;stroke-dasharray:6 4}
.iso{stroke:#b3261e;stroke-width:1.4;stroke-dasharray:4 4;fill:none}
</style>"""


class Sheet:
    def __init__(self, width, height, label):
        self.width, self.height, self.label = width, height, label
        self.parts = []

    def add(self, *parts):
        self.parts.extend(parts)

    def svg(self):
        body = "\n".join(self.parts)
        return (
            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {self.width} {self.height}" '
            f'width="{self.width}" height="{self.height}" role="img" aria-label="{self.label}">\n'
            f"{STYLE}\n"
            f'<rect width="{self.width}" height="{self.height}" fill="#fff"/>\n{body}\n</svg>\n'
        )


# Primitives. Coordinates are the wire ends each part connects to.

def wire(*points):
    d = "M" + " L".join(f"{x} {y}" for x, y in points)
    return f'<path class="w" d="{d}"/>'


def text(x, y, s, cls="", anchor="start"):
    c = f' class="{cls}"' if cls else ""
    a = f' text-anchor="{anchor}"' if anchor != "start" else ""
    return f'<text x="{x}" y="{y}"{c}{a}>{s}</text>'


def dot(x, y):
    return f'<circle class="solid" cx="{x}" cy="{y}" r="3"/>'


def terminal(x, y):
    return f'<circle class="b" cx="{x}" cy="{y}" r="5"/>'


def gnd(x, y):
    """Ground symbol hanging from (x, y)."""
    return (f'<path class="w" d="M{x} {y} V{y + 10} M{x - 11} {y + 10} H{x + 11} '
            f'M{x - 7} {y + 15} H{x + 7} M{x - 3} {y + 20} H{x + 3}"/>')


def resistor_h(x1, x2, y):
    """Horizontal resistor between x1 and x2."""
    mid = (x1 + x2) / 2
    return wire((x1, y), (mid - 18, y)) + wire((mid + 18, y), (x2, y)) + \
        f'<rect class="b" x="{mid - 18}" y="{y - 7}" width="36" height="14"/>'


def resistor_v(x, y1, y2):
    """Vertical resistor between y1 and y2."""
    mid = (y1 + y2) / 2
    return wire((x, y1), (x, mid - 16)) + wire((x, mid + 16), (x, y2)) + \
        f'<rect class="b" x="{x - 6}" y="{mid - 16}" width="12" height="32"/>'


def ceramic_v(x, y1, y2):
    mid = (y1 + y2) / 2
    return wire((x, y1), (x, mid - 4)) + wire((x, mid + 4), (x, y2)) + \
        f'<path class="w" d="M{x - 14} {mid - 4} H{x + 14} M{x - 14} {mid + 4} H{x + 14}"/>'


def electrolytic_v(x, y1, y2, plus_side="left"):
    """Polarised capacitor, + at the top (y1)."""
    mid = (y1 + y2) / 2
    px = x - 18 if plus_side == "left" else x + 18
    anchor = "end" if plus_side == "left" else "start"
    return wire((x, y1), (x, mid - 5)) + wire((x, mid + 7), (x, y2)) + \
        f'<path class="w" d="M{x - 14} {mid - 5} H{x + 14}"/>' + \
        f'<rect class="solid" x="{x - 14}" y="{mid + 2}" width="28" height="5"/>' + \
        text(px, mid - 8, "+", anchor=anchor)


def diode_h(x1, x2, y):
    """Diode pointing right: anode at x1, cathode at x2."""
    mid = (x1 + x2) / 2
    return wire((x1, y), (mid - 9, y)) + wire((mid + 9, y), (x2, y)) + \
        f'<polygon class="solid" points="{mid - 9},{y - 10} {mid - 9},{y + 10} {mid + 9},{y}"/>' + \
        f'<path class="w" d="M{mid + 9} {y - 10} V{y + 10}" style="stroke-width:2.2"/>'


def button_v(x, y1, y2):
    mid = (y1 + y2) / 2
    return wire((x, y1), (x, mid - 18)) + wire((x, mid + 18), (x, y2)) + \
        f'<path class="w" d="M{x} {mid + 18} L{x + 12} {mid - 14} M{x + 8} {mid} H{x + 20}"/>' + \
        f'<circle class="b" cx="{x}" cy="{mid - 18}" r="3"/><circle class="b" cx="{x}" cy="{mid + 18}" r="3"/>'


def arrow_in(x, y, label, sub="", side="right"):
    """Net label with an arrow pointing at the wire end (x, y)."""
    if side == "right":
        tri = f'<polygon class="solid" points="{x + 12},{y - 6} {x},{y} {x + 12},{y + 6}"/>'
        return tri + text(x + 18, y + 4, label, "ref") + (text(x + 18, y + 20, sub, "pin") if sub else "")
    tri = f'<polygon class="solid" points="{x - 12},{y - 6} {x},{y} {x - 12},{y + 6}"/>'
    return tri + text(x - 18, y + 4, label, "ref", "end") + (text(x - 18, y + 20, sub, "pin", "end") if sub else "")


def arrow_out(x, y, label, sub="", side="right"):
    """Net label with an arrow pointing away from the wire end (x, y)."""
    if side == "right":
        tri = f'<polygon class="solid" points="{x},{y - 6} {x + 12},{y} {x},{y + 6}"/>'
        return tri + text(x + 18, y + 4, label, "ref") + (text(x + 18, y + 20, sub, "pin") if sub else "")
    tri = f'<polygon class="solid" points="{x},{y - 6} {x - 12},{y} {x},{y + 6}"/>'
    return tri + text(x - 18, y + 4, label, "ref", "end") + (text(x - 18, y + 20, sub, "pin", "end") if sub else "")


def labels(x, y, ref, value, note="", anchor="start"):
    out = text(x, y, ref, "ref", anchor) + text(x, y + 16, value, anchor=anchor)
    return out + (text(x, y + 31, note, "pin", anchor) if note else "")


# Sheet 1: battery → fuse → divider tap → diode → DC-DC 5 V.

def sheet_power():
    s = Sheet(820, 380, "Аркуш 1: акумулятор до 80 вольт через запобіжник F1. Одразу за запобіжником дільник R1 330 кілоом і R2 10 кілоом з конденсатором C1 на вхід АЦП. Далі діод D1 і DC-DC JZ7015, налаштований на 5 вольт.")
    top, bot = 80, 320
    s.add(terminal(40, top), terminal(40, bot))
    s.add(text(20, top - 16, "+АКБ"), text(20, bot + 26, "−АКБ"), text(20, 205, "J1", "ref"))
    s.add(text(20, 222, "до 80 В", "pin"))
    # Fuse
    s.add(wire((45, top), (92, top)), f'<rect class="b" x="92" y="{top - 7}" width="36" height="14"/>',
          wire((92, top), (128, top)))
    s.add(text(110, top - 16, "F1", "ref", "middle"), text(110, top + 28, "1 A", anchor="middle"))
    # Divider tap right after the fuse
    tap = 190
    s.add(wire((128, top), (tap, top)), dot(tap, top))
    mid = 200
    s.add(resistor_v(tap, top, mid), labels(tap - 14, 128, "R1", "330k", "1%", "end"))
    s.add(resistor_v(tap, mid, bot), labels(tap - 14, 248, "R2", "10k", "1%", "end"))
    s.add(dot(tap, mid), dot(tap, bot))
    s.add(wire((tap, mid), (250, mid)), dot(250, mid), ceramic_v(250, mid, bot), dot(250, bot))
    s.add(labels(266, 250, "C1", "100nF", "кераміка"))
    s.add(wire((250, mid), (300, mid)), arrow_out(300, mid, "ADC", "до аркуша 2"))
    # Reverse polarity diode
    s.add(diode_h(tap, 410, top), text(300, top - 18, "D1", "ref", "middle"), text(300, top + 30, "SS1100", anchor="middle"))
    # DC-DC
    s.add(wire((410, top), (470, top)), wire((45, bot), (470, bot)))
    s.add('<rect class="b" x="470" y="50" width="150" height="300" rx="4"/>')
    s.add(text(478, top + 4, "IN+", "pin"), text(478, bot + 4, "IN−", "pin"))
    s.add(text(612, top + 4, "OUT+", "pin", "end"), text(612, bot + 4, "OUT−", "pin", "end"))
    s.add(text(545, 176, "U1", "ref", "middle"), text(545, 194, "JZ7015", anchor="middle"))
    s.add(text(545, 212, "DC-DC, вхід ≤ 80 В", "pin", "middle"), text(545, 240, "OUT = 5.0 В", "set", "middle"))
    s.add(wire((620, top), (690, top)), arrow_out(690, top, "+5 В", "до аркуша 2"))
    s.add(wire((620, bot), (690, bot)), dot(690, bot), gnd(690, bot), text(704, bot + 20, "GND", "ref"))
    return s


# Sheet 2a: ESP32-WROOM-32 module, bottom view, with its own 3.3 V regulator.

LEFT = ["GND", "IO23", "IO22", "TXD0", "RXD0", "IO21", "NC", "IO19", "IO18", "IO5", "IO17", "IO16", "IO4", "IO0"]  # pads 38..25
RIGHT = ["GND", "3V3", "EN", "SENSOR_VP", "SENSOR_VN", "IO34", "IO35", "IO32", "IO33", "IO25", "IO26", "IO27", "IO14", "IO12"]  # pads 1..14
BOTTOM = ["GND", "IO13", "SD2", "SD3", "CMD", "CLK", "SD0", "SD1", "IO15", "IO2"]  # pads 15..24, drawn right to left


def sheet_wroom():
    s = Sheet(1160, 620, "Аркуш 2а: модуль ESP32-WROOM-32 знизу, як на фото. AMS1117 робить 3.3 вольта з 5 вольт. Пад 2 3V3 з конденсаторами C2 і C3, пад 3 EN з підтяжкою R3, конденсатором C4 і кнопкою SW1. Пад 6 IO34 від дільника, пад 26 IO4 від оптопари. Земля на падах 1, 15 і 38.")
    x0, x1, y0, y1 = 300, 540, 40, 560
    pad_y = lambda i: 150 + i * 26
    s.add(f'<rect class="b" x="{x0}" y="{y0}" width="{x1 - x0}" height="{y1 - y0}" rx="4"/>')
    s.add(f'<rect x="{x0}" y="{y0}" width="{x1 - x0}" height="70" fill="rgba(179,38,30,.07)"/>')
    s.add(text(420, 66, "U3 ESP32-WROOM-32", "ref", "middle"), text(420, 86, "вид знизу, з боку падів", "pin", "middle"))
    s.add(f'<rect x="385" y="300" width="70" height="70" class="dash"/>')
    s.add(text(420, 331, "39 GND", "pin", "middle"), text(420, 346, "центр. пад", "pin", "middle"))
    for i, name in enumerate(LEFT):
        y = pad_y(i)
        s.add(wire((x0, y), (x0 - 10, y)), text(x0 + 8, y + 4, f"{38 - i} {name}", "pin"))
    for i, name in enumerate(RIGHT):
        y = pad_y(i)
        s.add(wire((x1, y), (x1 + 10, y)), text(x1 - 8, y + 4, f"{name} {i + 1}", "pin", "end"))
    for i, name in enumerate(BOTTOM):
        x = x1 - 30 - i * 20
        s.add(wire((x, y1), (x, y1 + 10)),
              f'<text class="pin" transform="translate({x + 4} {y1 - 8}) rotate(-90)">{15 + i} {name}</text>')
    # Ground pads 38, 1 and 15
    s.add(wire((x0, pad_y(0)), (x0 - 22, pad_y(0))), gnd(x0 - 22, pad_y(0)))
    s.add(wire((x1, pad_y(0)), (x1 + 12, pad_y(0))), gnd(x1 + 12, pad_y(0)))
    s.add(gnd(x1 - 30, y1 + 10))
    # 3V3: pad 2 up to the rail, rail to the regulator output
    rail = 120
    s.add(wire((x1, pad_y(1)), (575, pad_y(1)), (575, rail), (980, rail)), dot(575, rail))
    s.add(text(590, rail - 8, "3V3", "ref"))
    s.add(dot(620, rail), ceramic_v(620, rail, 166), gnd(620, 166), labels(636, 150, "C3", "100nF", "кераміка"))
    s.add(dot(880, rail), electrolytic_v(880, rail, 166), gnd(880, 166), labels(896, 150, "C2", "100µF", "≥ 6.3 В"))
    # EN: pull-up, delay capacitor, optional button
    en = pad_y(2)
    s.add(dot(720, rail), resistor_v(720, rail, en), labels(734, 148, "R3", "10k"))
    s.add(wire((x1, en), (840, en)), dot(720, en), text(560, en - 6, "EN", "pin"))
    s.add(dot(780, en), electrolytic_v(780, en, 236, "right"), gnd(780, 236), labels(764, 236, "C4", "1µF", "", "end"))
    s.add(button_v(840, en, 236), gnd(840, 236), labels(866, 220, "SW1", "RST", "опційно"))
    # IO34 from the divider
    s.add(wire((x1, pad_y(5)), (600, pad_y(5))), arrow_in(600, pad_y(5), "ADC", "з аркуша 1 (R1/R2)"))
    # AMS1117 in SOT-223, marking up, legs down: 1 GND, 2 OUT, 3 IN
    s.add(text(980, 18, "вид на напис, ніжки вниз", "pin", "middle"))
    s.add('<rect class="b" x="950" y="26" width="60" height="14"/>', text(1016, 37, "таб = OUT", "pin"))
    s.add('<rect class="b" x="920" y="40" width="120" height="44" rx="2"/>')
    s.add(text(980, 58, "U2", "ref", "middle"), text(980, 76, "AMS1117-3.3", anchor="middle"))
    for x, leg in ((940, "1"), (980, "2"), (1020, "3")):
        s.add(text(x + 4, 98, leg, "pin"))
    s.add(wire((940, 84), (940, 96)), gnd(940, 96))
    s.add(wire((980, 84), (980, rail)), dot(980, rail))
    s.add(wire((1020, 84), (1020, 104), (1050, 104)), arrow_in(1050, 104, "+5 В", "з аркуша 1"))
    # IO4 from the optocoupler
    y4 = pad_y(LEFT.index("IO4"))
    s.add(wire((x0, y4), (200, y4)), arrow_in(200, y4, "GPIO4", "з аркуша 3", "left"))
    return s


def board_sheet(label, name, sub, pins, extra=lambda s: None, width=900, height=420):
    """A ready-made board: only the pins this project uses."""
    s = Sheet(width, height, label)
    s.add('<rect class="b" x="330" y="60" width="240" height="300" rx="6"/>')
    s.add(text(450, 92, name, "ref", "middle"), text(450, 110, sub, "pin", "middle"))
    for side, y, pin, net, note, kind in pins:
        if side == "left":
            s.add(wire((330, y), (200, y)), text(338, y + 4, pin, "pin"))
            s.add(arrow_in(200, y, net, note, "left"))
        else:
            s.add(wire((570, y), (700, y)), text(562, y + 4, pin, "pin", "end"))
            if kind == "gnd":
                s.add(dot(700, y), gnd(700, y), text(714, y + 22, net, "ref"))
            else:
                s.add(arrow_in(700, y, net, note))
    extra(s)
    return s


def sheet_devkit():
    return board_sheet(
        "Аркуш 2б: плата ESP32 DevKit. 5 вольт з аркуша 1 на пін VIN, земля на GND, вихід дільника на GPIO34, вихід оптопари на GPIO4.",
        "U3 ESP32 DevKit", "30 або 38 пінів, будь-яка",
        [
            ("right", 140, "VIN / 5V", "+5 В", "з аркуша 1", ""),
            ("right", 220, "GND", "GND", "", "gnd"),
            ("left", 140, "GPIO34 (D34)", "ADC", "з аркуша 1 (R1/R2)", ""),
            ("left", 220, "GPIO4 (D4)", "GPIO4", "з аркуша 3", ""),
        ],
    )


def sheet_d1_mini():
    def inside(s):
        # The board's own divider on A0, already soldered on the D1 mini
        s.add('<rect x="345" y="250" width="210" height="96" class="dash"/>')
        s.add(text(450, 266, "на платі вже є:", "pin", "middle"))
        s.add(text(450, 284, "A0 → 220k → ADC → 100k → GND", "pin", "middle"))
        s.add(text(450, 302, "чип міряє 0…1 В,", "pin", "middle"))
        s.add(text(450, 318, "пін A0 — 0…3.2 В", "pin", "middle"))
    return board_sheet(
        "Аркуш 2в: плата Wemos D1 mini з ESPHome. 5 вольт з аркуша 1 на пін 5V, земля на G, вихід дільника на A0, вихід оптопари на D5.",
        "U3 Wemos D1 mini", "ESP8266, прошивка ESPHome",
        [
            ("right", 140, "5V", "+5 В", "з аркуша 1", ""),
            ("right", 220, "G", "GND", "", "gnd"),
            ("left", 140, "A0", "ADC", "з аркуша 1 (R1/R2)", ""),
            ("left", 220, "D5 (GPIO14)", "D5", "з аркуша 3", ""),
        ],
        inside,
    )


# Sheet 3: the 220 V sensor, the same for every board.

def sheet_mains():
    s = Sheet(1000, 300, "Аркуш 3: 220 вольт через чотири резистори R4–R7 по 82 кілоом на світлодіод оптопари U4 PC817, діод D2 1N4148 зустрічно паралельно. Вихід оптопари з підтяжкою R8 100 кілоом до 3V3 і конденсатором C5 3.3 мікрофаради на землю, далі на GPIO4 або D5.")
    s.add('<defs><marker id="arr" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto">'
          '<path d="M0 0 L8 4 L0 8 Z" fill="#1d2326"/></marker></defs>')
    s.add('<rect class="zone" x="8" y="48" width="497" height="226"/>', text(18, 68, "220 В — небезпечно для життя", "warn"))
    s.add(terminal(40, 120), terminal(40, 220), text(18, 124, "L", "ref"), text(16, 224, "N", "ref"), text(40, 174, "J2", "ref", "middle"))
    s.add(wire((45, 220), (430, 220)))
    s.add(resistor_h(45, 135, 120), resistor_h(135, 205, 120), resistor_h(205, 275, 120), resistor_h(275, 345, 120))
    s.add(wire((345, 120), (430, 120)))
    for ref, x in (("R4", 90), ("R5", 170), ("R6", 240), ("R7", 310)):
        s.add(text(x, 104, ref, "ref", "middle"), text(x, 146, "82k", anchor="middle"))
    s.add(text(205, 166, "усі 0.25 Вт, разом 270–390 кОм", "pin", "middle"))
    # D2 antiparallel across the LED, cathode up
    s.add('<polygon class="solid" points="370,180 390,180 380,162"/><path class="w" d="M370 162 H390" style="stroke-width:2.2"/>')
    s.add(wire((380, 120), (380, 162)), wire((380, 180), (380, 220)), dot(380, 120), dot(380, 220))
    s.add(text(364, 196, "D2", "ref", "end"), text(364, 212, "1N4148", anchor="end"))
    # U4 PC817
    s.add('<rect class="b" x="430" y="90" width="150" height="160" rx="3"/>', text(505, 84, "U4 PC817", "ref", "middle"))
    s.add(text(437, 112, "1", "pin"), text(437, 242, "2", "pin"), text(573, 112, "4", "pin", "end"), text(573, 242, "3", "pin", "end"))
    s.add('<path class="w" d="M430 120 H465 V160 M465 178 V220 H430"/>')
    s.add('<polygon class="solid" points="455,160 475,160 465,178"/><path class="w" d="M455 178 H475" style="stroke-width:2"/>')
    s.add('<path class="w" d="M480 160 L498 154" marker-end="url(#arr)"/><path class="w" d="M480 172 L498 166" marker-end="url(#arr)"/>')
    s.add('<path class="iso" d="M505 96 V244"/>')
    s.add('<path class="w" d="M528 150 V190" style="stroke-width:2.6"/>')
    s.add('<path class="w" d="M528 162 L548 146 V120 H580 M528 178 L548 194 V220 H580"/>')
    s.add('<polygon class="solid" points="548,194 537,191 542,184"/>')
    # Output: pull-up and the 50 Hz filter
    s.add(wire((580, 120), (840, 120)), wire((580, 220), (730, 220)))
    s.add(resistor_v(650, 40, 120), text(650, 32, "3V3", "ref", "middle"), labels(662, 74, "R8", "100k"), dot(650, 120))
    s.add(electrolytic_v(730, 120, 220), dot(730, 120), labels(752, 160, "C5", "3.3µF", "електроліт ≥ 6.3 В"))
    s.add(dot(650, 220), gnd(650, 220))
    s.add(arrow_out(840, 120, "GPIO4 / D5", "до аркуша 2"))
    s.add(text(505, 266, "ізоляція: ≥ 6 мм між сторонами", "pin", "middle"))
    return s


SHEETS = {
    "sheet-1-power.svg": sheet_power,
    "sheet-2a-wroom.svg": sheet_wroom,
    "sheet-2b-devkit.svg": sheet_devkit,
    "sheet-2c-d1-mini.svg": sheet_d1_mini,
    "sheet-3-mains.svg": sheet_mains,
}

if __name__ == "__main__":
    for name, draw in SHEETS.items():
        (HERE / name).write_text(draw().svg(), encoding="utf-8")
        print(name)
