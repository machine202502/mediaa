# Build environment for the Linux version of MediaA.
# Ubuntu 24.04: the PipeWire bindings used for Wayland capture need PipeWire >= 1.0.
# The binary needs glibc >= 2.39 (Ubuntu 24.04+, Debian 13+, Fedora 40+, WSL Ubuntu-24.04).
FROM ubuntu:24.04

ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential ca-certificates curl git pkg-config zip unzip tar xz-utils \
        nasm yasm cmake ninja-build autoconf automake autoconf-archive libtool python3 bison flex \
        clang libclang-dev \
        libpipewire-0.3-dev libspa-0.2-dev libgbm-dev libegl1-mesa-dev libgl1-mesa-dev \
        libxcb1-dev libxcb-randr0-dev libxcb-shm0-dev libxcb-xfixes0-dev libxcb-shape0-dev \
        libx11-dev libxi-dev libxtst-dev libxrandr-dev libxkbcommon-dev libwayland-dev \
        libasound2-dev libdbus-1-dev \
    && rm -rf /var/lib/apt/lists/*

RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
ENV PATH=/root/.cargo/bin:$PATH

# vcpkg pinned to the same commit as the Windows build (scripts/vcpkg-commit.txt).
ARG VCPKG_COMMIT
RUN git clone https://github.com/microsoft/vcpkg /opt/vcpkg \
    && cd /opt/vcpkg && git checkout "$VCPKG_COMMIT" \
    && ./bootstrap-vcpkg.sh -disableMetrics
ENV VCPKG_ROOT=/opt/vcpkg

WORKDIR /src
