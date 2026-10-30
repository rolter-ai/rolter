# /// script
# requires-python = ">=3.10"
# dependencies = ["onnx==1.17.0"]
# ///
"""Regenerate the onnx/1 test fixture of the local semantic guard classifier.

The model is a few hundred bytes and needs no training: it answers the logits
of `[safe, prompt_injection]` from whether the token `jailbreak` (id 10) is
among the attended input ids, so a test can tell a flagged text from a benign
one and a truncated window from a whole one.

    uv run --python 3.12 --script make_fixture.py

then run `just fmt-docs` (it reformats tokenizer.json) and update the two
`sha256` pins in manifest.json (`sha256sum model.onnx tokenizer.json`). See
#3443.
"""

import json
import pathlib

from onnx import ModelProto, TensorProto, checker, helper

HERE = pathlib.Path(__file__).parent
JAILBREAK = 10

graph = helper.make_graph(
    nodes=[
        helper.make_node("Constant", [], ["needle"], value=helper.make_tensor("needle", TensorProto.INT64, [], [JAILBREAK])),
        helper.make_node("Equal", ["input_ids", "needle"], ["found"]),
        helper.make_node("Cast", ["found"], ["found_f"], to=TensorProto.FLOAT),
        helper.make_node("Cast", ["attention_mask"], ["mask_f"], to=TensorProto.FLOAT),
        helper.make_node("Mul", ["found_f", "mask_f"], ["attended"]),
        helper.make_node("ReduceMax", ["attended"], ["hit"], axes=[1], keepdims=1),
        helper.make_node("MatMul", ["hit", "weights"], ["scaled"]),
        helper.make_node("Add", ["scaled", "bias"], ["logits"]),
    ],
    name="rolter-onnx-fixture",
    inputs=[
        helper.make_tensor_value_info("input_ids", TensorProto.INT64, [1, "sequence"]),
        helper.make_tensor_value_info("attention_mask", TensorProto.INT64, [1, "sequence"]),
    ],
    outputs=[helper.make_tensor_value_info("logits", TensorProto.FLOAT, [1, 2])],
    initializer=[
        helper.make_tensor("weights", TensorProto.FLOAT, [1, 2], [-4.0, 4.0]),
        helper.make_tensor("bias", TensorProto.FLOAT, [2], [2.0, -2.0]),
    ],
)
model = helper.make_model(graph, opset_imports=[helper.make_opsetid("", 13)], producer_name="rolter")
model.ir_version = 8
checker.check_model(model)
(HERE / "model.onnx").write_bytes(model.SerializeToString())

# a valid graph whose selected output cannot be extracted as f32
wide = ModelProto()
wide.CopyFrom(model)
wide.graph.node[-1].output[0] = "float_logits"
wide.graph.node.append(helper.make_node("Cast", ["float_logits"], ["logits"], to=TensorProto.DOUBLE))
wide.graph.output[0].type.tensor_type.elem_type = TensorProto.DOUBLE
checker.check_model(wide)
(HERE / "model-f64.onnx").write_bytes(wide.SerializeToString())

# the empty-sequence graph uses ReduceSum's identity, unlike the needle model's max
empty_graph = helper.make_graph(
    nodes=[
        helper.make_node("ReduceSum", ["input_ids", "axes"], ["sum"], keepdims=1),
        helper.make_node("Cast", ["sum"], ["sum_f"], to=TensorProto.FLOAT),
        helper.make_node("MatMul", ["sum_f", "zeros"], ["scaled"]),
        helper.make_node("Add", ["scaled", "bias"], ["logits"]),
    ],
    name="rolter-empty-sequence-fixture",
    inputs=[helper.make_tensor_value_info("input_ids", TensorProto.INT64, [1, "sequence"])],
    outputs=[helper.make_tensor_value_info("logits", TensorProto.FLOAT, [1, 2])],
    initializer=[
        helper.make_tensor("axes", TensorProto.INT64, [1], [1]),
        helper.make_tensor("zeros", TensorProto.FLOAT, [1, 2], [0.0, 0.0]),
        helper.make_tensor("bias", TensorProto.FLOAT, [2], [2.0, -2.0]),
    ],
)
empty = helper.make_model(empty_graph, opset_imports=[helper.make_opsetid("", 13)], producer_name="rolter")
empty.ir_version = 8
checker.check_model(empty)
(HERE / "model-empty.onnx").write_bytes(empty.SerializeToString())

special = lambda token, id_: {
    "id": id_, "content": token, "single_word": False, "lstrip": False,
    "rstrip": False, "normalized": False, "special": True,
}
words = ["what", "is", "the", "capital", "of", "france", "jailbreak", "ignore", "instructions", "hello"]
vocab = {"[UNK]": 0, "[CLS]": 1, "[SEP]": 2, "[PAD]": 3}
vocab.update({word: index + 4 for index, word in enumerate(words)})
assert vocab["jailbreak"] == JAILBREAK
tokenizer = {
    "version": "1.0",
    "truncation": None,
    "padding": None,
    "added_tokens": [special("[UNK]", 0), special("[CLS]", 1), special("[SEP]", 2), special("[PAD]", 3)],
    "normalizer": {"type": "Lowercase"},
    "pre_tokenizer": {"type": "Whitespace"},
    "post_processor": {
        "type": "TemplateProcessing",
        "single": [
            {"SpecialToken": {"id": "[CLS]", "type_id": 0}},
            {"Sequence": {"id": "A", "type_id": 0}},
            {"SpecialToken": {"id": "[SEP]", "type_id": 0}},
        ],
        "pair": [
            {"SpecialToken": {"id": "[CLS]", "type_id": 0}},
            {"Sequence": {"id": "A", "type_id": 0}},
            {"SpecialToken": {"id": "[SEP]", "type_id": 0}},
            {"Sequence": {"id": "B", "type_id": 1}},
            {"SpecialToken": {"id": "[SEP]", "type_id": 1}},
        ],
        "special_tokens": {
            "[CLS]": {"id": "[CLS]", "ids": [1], "tokens": ["[CLS]"]},
            "[SEP]": {"id": "[SEP]", "ids": [2], "tokens": ["[SEP]"]},
        },
    },
    "decoder": None,
    "model": {"type": "WordLevel", "vocab": vocab, "unk_token": "[UNK]"},
}
(HERE / "tokenizer.json").write_text(json.dumps(tokenizer, indent=2) + "\n")
