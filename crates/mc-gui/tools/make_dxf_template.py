"""Makes src/drawing_template.dxf: an empty AutoCAD 2010 drawing in micrometres made by ezdxf, with markers
where mc-operator adds its layers (@LAYERS@), entities (@ENTITIES@), objects (@OBJECTS@), the entries of the
root dictionary (@ROOT@), the raster classes (@CLASSES@) and the next free handle (@HANDSEED@).
Run once when the template must change:  python tools/make_dxf_template.py  (needs: pip install ezdxf)"""
import io, pathlib, ezdxf

d = ezdxf.new("R2010", setup=False)
d.units = 13  # micrometres
d.header["$MEASUREMENT"] = 1
d.header["$LUNITS"] = 2
d.header["$LUPREC"] = 4
s = io.StringIO()
d.write(s)
t = s.getvalue().replace("\r\n", "\n")
L = t.split("\n")

def find(seq, start=0):
    for i in range(start, len(L) - len(seq)):
        if L[i:i + len(seq)] == seq:
            return i
    raise SystemExit(f"not found: {seq}")

# handles the writer needs
root = d.rootdict.dxf.handle
msp = d.block_records.get("*Model_Space").dxf.handle
layer_table = d.layers.head.dxf.handle
zero = d.layers.get("0")
plot = zero.dxf.plotstyle_handle
material = zero.dxf.material_handle
seed = d.header["$HANDSEED"]

i = find(["$HANDSEED", "  5"]); L[i + 2] = "@HANDSEED@"
i = find(["  0", "SECTION", "  2", "CLASSES"]); L.insert(i + 4, "@CLASSES@")
i = find(["  2", "LAYER"]); j = find(["  0", "ENDTAB"], i); L.insert(j, "@LAYERS@")
i = find(["  2", "ENTITIES"]); j = find(["  0", "ENDSEC"], i); L.insert(j, "@ENTITIES@")
i = find(["  5", root, "330", "0", "100", "AcDbDictionary"]); L.insert(i + 8, "@ROOT@")  # after 281 / 1
i = find(["  2", "OBJECTS"]); j = find(["  0", "ENDSEC"], i); L.insert(j, "@OBJECTS@")
out = pathlib.Path(__file__).resolve().parent.parent / "src" / "drawing_template.dxf"
out.write_text("\n".join(L), newline="\n")
print(f"root={root} msp={msp} layers={layer_table} plot={plot} material={material} seed={seed}")
