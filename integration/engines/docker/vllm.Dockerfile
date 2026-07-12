FROM ghcr.io/astral-sh/uv:0.9.26 AS uv
FROM nvidia/cuda:12.8.1-cudnn-runtime-ubuntu24.04

COPY --from=uv /uv /uvx /bin/
ENV UV_PYTHON_INSTALL_DIR=/opt/uv/python \
    UV_TOOL_DIR=/opt/uv/tools

ENTRYPOINT ["uvx", "--python", "3.12", "--from", "vllm==0.24.0", "vllm", "serve"]
