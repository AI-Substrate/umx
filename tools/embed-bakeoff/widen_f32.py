"""Write an F32 copy of a BF16 GGUF (ws004 s16): every BF16 tensor widened to F32, everything else copied as is.

BF16 is the top 16 bits of an F32, so the widening is exact: each weight keeps its value, only its storage grows.
general.file_type becomes ALL_F32 (0); all other metadata is copied unchanged. Needs `gguf` and `numpy`
(pip install gguf numpy). The copy logic follows gguf-py's own gguf_new_metadata.py.

    python widen_f32.py <in BF16.gguf> <out F32.gguf>
"""

import sys

import gguf
import numpy as np


def main() -> None:
    src, dst = sys.argv[1], sys.argv[2]
    reader = gguf.GGUFReader(src)
    arch = reader.fields[gguf.Keys.General.ARCHITECTURE].contents()
    writer = gguf.GGUFWriter(dst, arch=arch, endianess=reader.endianess)
    alignment = reader.fields.get(gguf.Keys.General.ALIGNMENT)
    if alignment is not None:
        writer.data_alignment = alignment.contents()

    for field in reader.fields.values():
        if field.name == gguf.Keys.General.ARCHITECTURE or field.name.startswith("GGUF."):
            continue
        value = field.contents()
        if field.name == gguf.Keys.General.FILE_TYPE:
            value = int(gguf.LlamaFileType.ALL_F32)
        val_type = field.types[0]
        sub_type = field.types[-1] if val_type == gguf.GGUFValueType.ARRAY else None
        writer.add_key_value(field.name, value, val_type, sub_type=sub_type)

    tensors = []
    for t in reader.tensors:
        shape = tuple(int(d) for d in reversed(t.shape))
        if t.tensor_type == gguf.GGMLQuantizationType.BF16:
            bits = np.ascontiguousarray(t.data).view(np.uint16).reshape(shape)
            data = (bits.astype(np.uint32) << 16).view(np.float32)
        elif t.tensor_type == gguf.GGMLQuantizationType.F32:
            data = np.ascontiguousarray(t.data).view(np.float32).reshape(shape)
        else:
            sys.exit(f"{t.name}: {t.tensor_type.name} is neither BF16 nor F32")
        tensors.append((t.name, data))
        writer.add_tensor_info(t.name, data.shape, data.dtype, data.nbytes, gguf.GGMLQuantizationType.F32)

    writer.write_header_to_file()
    writer.write_kv_data_to_file()
    writer.write_ti_data_to_file()
    for _, data in tensors:
        writer.write_tensor_data(data)
    writer.close()


if __name__ == "__main__":
    main()
