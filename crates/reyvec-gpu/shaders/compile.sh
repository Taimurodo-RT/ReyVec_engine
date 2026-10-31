#!/usr/bin/env sh
# Rebuilds the precompiled SPIR-V next to the GLSL sources.
# Stage 1 keeps .spv in git so Windows builds need no shader compiler.
# From stage 2 shaders move to Slang and are compiled by the build.
set -eu
cd "$(dirname "$0")"
for f in triangle.vert triangle.frag; do
    glslangValidator -V --target-env vulkan1.3 -o "$f.spv" "$f" >/dev/null
done
