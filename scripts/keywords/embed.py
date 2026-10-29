"""Makes crates/lf-ai/data/keywords.f16 from crates/lf-ai/data/keywords.txt: each keyword's
CLIP ViT-B/32 text embedding (OpenAI, MIT), averaged over a few prompt templates and scaled
to unit length, as little-endian float16, 512 per keyword, in the file's order.

The app only ships the result and runs CLIP's image model; the text model and tokenizer are
needed here alone. Get them from Xenova/clip-vit-base-patch32 at commit
6b7c523703ae26e39be4b03774966f8571c7f523 (onnx/text_model.onnx, tokenizer.json) and run:

    python -m venv venv && venv/bin/pip install onnxruntime numpy tokenizers
    venv/bin/python -I scripts/keywords/embed.py --model text_model.onnx --tokenizer tokenizer.json
"""
import argparse
import os

import numpy as np
import onnxruntime as ort
from tokenizers import Tokenizer

TEMPLATES = [
    "a photo of {}.",
    "a good photo of {}.",
    "a close-up photo of {}.",
    "a bright photo of {}.",
    "a dark photo of {}.",
    "a blurry photo of {}.",
]

here = os.path.dirname(os.path.abspath(__file__))
data = os.path.normpath(os.path.join(here, "..", "..", "crates", "lf-ai", "data"))
ap = argparse.ArgumentParser()
ap.add_argument("--model", required=True)
ap.add_argument("--tokenizer", required=True)
a = ap.parse_args()

words = []
for line in open(os.path.join(data, "keywords.txt"), encoding="utf-8"):
    line = line.rstrip("\n")
    if not line or line.startswith("#"):
        continue
    keyword, phrase = line.split("\t")
    words.append((keyword, phrase))

tok = Tokenizer.from_file(a.tokenizer)
eot = tok.token_to_id("<|endoftext|>")
sess = ort.InferenceSession(a.model, providers=["CPUExecutionProvider"])


def embed(texts):
    ids = [tok.encode(t).ids[:77] for t in texts]
    n = max(map(len, ids))
    # Padding with the end token is harmless: the model pools at the first one.
    batch = np.array([i + [eot] * (n - len(i)) for i in ids], dtype=np.int64)
    e = sess.run(["text_embeds"], {"input_ids": batch})[0]
    return e / np.linalg.norm(e, axis=1, keepdims=True)


out = []
for keyword, phrase in words:
    mean = embed([t.format(phrase) for t in TEMPLATES]).mean(axis=0)
    out.append(mean / np.linalg.norm(mean))
table = np.stack(out).astype("<f2")
table.tofile(os.path.join(data, "keywords.f16"))
print(f"{len(words)} keywords, {table.shape[1]} numbers each")
