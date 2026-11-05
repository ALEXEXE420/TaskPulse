"""Wandelt TaskPulse.ico in TaskPulse.res (Windows-Ressource, ohne C-Präprozessor nötig)."""
import struct, sys

def entry(rtype, rid, data, lang=0x0409):
    hdr = struct.pack('<IIHHHHIHHII', len(data), 32, 0xFFFF, rtype, 0xFFFF, rid, 0, 0x1030, lang, 0, 0)
    pad = (-len(data)) % 4
    return hdr + data + b'\0' * pad

ico = open('TaskPulse.ico', 'rb').read()
_, _, n = struct.unpack_from('<HHH', ico, 0)
out = struct.pack('<IIHHHHIHHII', 0, 32, 0xFFFF, 0, 0xFFFF, 0, 0, 0, 0, 0, 0)  # leerer Kopf
group = struct.pack('<HHH', 0, 1, n)
for i in range(n):
    w, h, cc, r, planes, bpp, size, off = struct.unpack_from('<BBBBHHII', ico, 6 + 16 * i)
    out += entry(3, i + 1, ico[off:off + size])
    group += struct.pack('<BBBBHHIH', w, h, cc, r, planes, bpp, size, i + 1)
out += entry(14, 1, group)
open('TaskPulse.res', 'wb').write(out)
print('TaskPulse.res', len(out), 'Bytes,', n, 'Bilder')
