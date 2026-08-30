"""Pack GeoNames and Natural Earth into what openfoto bundles. Driven by build-geodata.sh."""
import io, json, os, struct

work, root = os.environ["WORK"], os.environ["ROOT"]

admin1 = {}
for line in io.open(f"{work}/admin1CodesASCII.txt", encoding="utf-8"):
    f = line.rstrip("\n").split("\t")
    if len(f) >= 2:
        admin1[f[0]] = f[1]                      # "GR.13" -> "South Aegean"

cname = {}
for line in io.open(f"{work}/countryInfo.txt", encoding="utf-8"):
    if line.startswith("#"):
        continue
    f = line.rstrip("\n").split("\t")
    if len(f) >= 5:
        cname[f[0]] = f[4]                       # "GR" -> "Greece"

rows = []
for line in io.open(f"{work}/cities1000.txt", encoding="utf-8"):
    f = line.rstrip("\n").split("\t")
    if len(f) < 15:
        continue
    name, ascii_name, lat, lon, cc, a1, pop = f[1], f[2], f[4], f[5], f[8], f[10], f[14]
    if not (name and lat and lon and cc):
        continue
    # GeoNames' ASCII column is what makes "Fira" find "Firá" and "Reykjavik" find
    # "Reykjavík" — people do not type accents. Stored only when it differs.
    alt = ascii_name if ascii_name and ascii_name != name else ""
    rows.append((name, alt, float(lat), float(lon), cc,
                 admin1.get(f"{cc}.{a1}", ""), int(pop or 0)))

# Biggest first. A tie between a village and a city an equal distance away should read
# as the city, and searching by name should offer the place people mean first.
rows.sort(key=lambda r: -r[6])

countries = sorted({r[4] for r in rows})
regions = sorted({r[5] for r in rows})
ci = {c: i for i, c in enumerate(countries)}
ri = {r: i for i, r in enumerate(regions)}

buf = bytearray(b"OFGEO2\n")
buf += struct.pack("<I", len(countries))
for c in countries:
    cb, nb = c.encode(), cname.get(c, c).encode()
    buf += struct.pack("<B", len(cb)) + cb + struct.pack("<B", len(nb)) + nb
buf += struct.pack("<I", len(regions))
for r in regions:
    rb = r.encode()
    buf += struct.pack("<H", len(rb)) + rb
buf += struct.pack("<I", len(rows))
for name, alt, lat, lon, cc, reg, _pop in rows:
    nb = name.encode()[:255]
    ab = alt.encode()[:255]
    buf += struct.pack("<B", len(nb)) + nb
    buf += struct.pack("<B", len(ab)) + ab
    buf += struct.pack("<iiHH", round(lat * 1e4), round(lon * 1e4), ci[cc], ri[reg])

open(f"{root}/crates/openfoto-core/data/places.bin", "wb").write(buf)
print(f"places.bin: {len(rows)} places, {len(countries)} countries, "
      f"{len(regions)} regions, {len(buf)/1048576:.2f} MB")


def outlines(src, dp=2):
    """Coordinate rings only. Natural Earth's properties are most of the file and the
    map needs none of them; rounding then collapses neighbouring points, which is most
    of the rest and changes nothing on screen."""
    g = json.load(io.open(src, encoding="utf-8"))
    rings = []
    for feat in g["features"]:
        geom = feat["geometry"]
        polys = geom["coordinates"] if geom["type"] == "MultiPolygon" else [geom["coordinates"]]
        for poly in polys:
            for ring in poly:
                pts, last = [], None
                for x, y in ring:
                    p = [round(x, dp), round(y, dp)]
                    if p != last:
                        pts.append(p)
                        last = p
                if len(pts) >= 4:
                    rings.append(pts)
    return rings


for name, src in [("world110", "ne110.geojson"), ("world50", "ne50.geojson")]:
    rings = outlines(f"{work}/{src}")
    out = json.dumps(rings, separators=(",", ":"))
    io.open(f"{root}/apps/desktop/dist/{name}.json", "w").write(out)
    print(f"{name}.json: {len(rings)} rings, {sum(len(r) for r in rings)} points, "
          f"{len(out)/1024:.0f} KB")
