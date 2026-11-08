import json
from pathlib import Path
p = Path("docs/logs")
r = json.loads((p / "run.json").read_text())
b = json.loads((p / "bus.json").read_text())
assert r["reason"] == "Halted", r["reason"]
assert bytes(r["uart"]) == b"Rust RV32EC OK\n", r["uart"]
assert any(x["kind"] == "Write" and x["address"] == 0x20000004 and x["value"] == 0x123456a2 for x in b), b
assert r["stats"]["instructions"] > 0
print(f'PASS: {r["stats"]["instructions"]} instructions; RESULT=0x123456a2; UART=Rust RV32EC OK')
