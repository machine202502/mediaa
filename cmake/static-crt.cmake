# CMake builds of native dependencies (llama.cpp) link the static C runtime on MSVC, like the
# rest of the single-file executable (`+crt-static`, the vcpkg `x64-windows-static-release`
# triplet). No effect with other compilers.
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")
