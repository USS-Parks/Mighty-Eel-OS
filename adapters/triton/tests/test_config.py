"""Unit tests for ``adapters.triton.config``.

Cover the dataclass defaults, the model-path / base-url builders, and
the ``supports_text_io`` capability cross-check that gates the high
level generate surface.
"""

from __future__ import annotations

import pytest

from adapters.triton.config import TritonConfig


class TestDefaults:
    def test_defaults_are_loopback_and_kserve(self) -> None:
        cfg = TritonConfig()
        assert cfg.host == "127.0.0.1"
        assert cfg.port == 8000
        assert cfg.grpc_port == 8001
        assert cfg.use_ssl is False
        assert cfg.model_name == "model"
        assert cfg.model_version == ""
        assert cfg.timeout_ms == 60000
        assert cfg.stream_timeout_ms == 300000
        assert cfg.declares_batching is True
        assert cfg.declares_embedding is False

    def test_text_io_defaults_off(self) -> None:
        cfg = TritonConfig()
        assert cfg.supports_text_io is False


class TestBaseUrl:
    def test_http_default(self) -> None:
        assert TritonConfig().base_url == "http://127.0.0.1:8000"

    def test_https_when_ssl(self) -> None:
        cfg = TritonConfig(host="triton.local", port=8443, use_ssl=True)
        assert cfg.base_url == "https://triton.local:8443"

    def test_trailing_slash_is_not_introduced(self) -> None:
        cfg = TritonConfig()
        assert not cfg.base_url.endswith("/")


class TestModelPath:
    def test_no_version(self) -> None:
        cfg = TritonConfig(model_name="resnet50")
        assert cfg.model_path() == "/v2/models/resnet50"

    def test_with_version(self) -> None:
        cfg = TritonConfig(model_name="resnet50", model_version="3")
        assert cfg.model_path() == "/v2/models/resnet50/versions/3"

    def test_empty_version_is_treated_as_latest(self) -> None:
        cfg = TritonConfig(model_name="resnet50", model_version="")
        # No /versions/ segment => Triton picks the latest.
        assert "/versions/" not in cfg.model_path()


class TestSupportsTextIo:
    def test_both_bytes_wired(self) -> None:
        cfg = TritonConfig(
            input_tensor_name="text_input",
            output_tensor_name="text_output",
            input_datatype="BYTES",
            output_datatype="BYTES",
        )
        assert cfg.supports_text_io is True

    def test_one_tensor_missing(self) -> None:
        cfg = TritonConfig(
            input_tensor_name="text_input",
            output_tensor_name="",
            input_datatype="BYTES",
            output_datatype="BYTES",
        )
        assert cfg.supports_text_io is False

    @pytest.mark.parametrize(
        ("input_dt", "output_dt"),
        [
            ("FP32", "BYTES"),
            ("BYTES", "FP16"),
            ("INT8", "INT8"),
        ],
    )
    def test_wrong_dtype_disables_text_io(self, input_dt: str, output_dt: str) -> None:
        cfg = TritonConfig(
            input_tensor_name="text_input",
            output_tensor_name="text_output",
            input_datatype=input_dt,
            output_datatype=output_dt,
        )
        assert cfg.supports_text_io is False

    def test_lowercase_bytes_accepted(self) -> None:
        # Operators sometimes type the datatype in lower case.
        cfg = TritonConfig(
            input_tensor_name="text_input",
            output_tensor_name="text_output",
            input_datatype="bytes",
            output_datatype="bytes",
        )
        assert cfg.supports_text_io is True


class TestFromDict:
    def test_known_and_extra_split(self) -> None:
        cfg = TritonConfig.from_dict({
            "model_name": "yolo",
            "model_version": "7",
            "max_input_len": 8192,
            "operator_custom": "yes",
            "another_extra": 42,
        })
        assert cfg.model_name == "yolo"
        assert cfg.model_version == "7"
        assert cfg.max_input_len == 8192
        assert cfg.extra_options == {
            "operator_custom": "yes",
            "another_extra": 42,
        }

    def test_empty_dict_yields_defaults(self) -> None:
        cfg = TritonConfig.from_dict({})
        assert cfg == TritonConfig()
