FROM ghcr.io/astral-sh/uv:0.9.26 AS uv
FROM nvidia/cuda:12.8.1-cudnn-runtime-ubuntu24.04

COPY --from=uv /uv /uvx /bin/
ENV UV_PYTHON_INSTALL_DIR=/opt/uv/python \
    UV_TOOL_DIR=/opt/uv/tools

ENTRYPOINT ["uv", "run", "--isolated", "--python", "3.12", "--with", "sglang[all]==0.5.12", "python", "-m", "sglang.launch_server"]
