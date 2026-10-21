# macOS universal app, arm64 half: static libraries, release builds only, for macOS 11 and newer.
set(VCPKG_TARGET_ARCHITECTURE arm64)
set(VCPKG_CRT_LINKAGE dynamic)
set(VCPKG_LIBRARY_LINKAGE static)
set(VCPKG_CMAKE_SYSTEM_NAME Darwin)
set(VCPKG_OSX_ARCHITECTURES arm64)
set(VCPKG_OSX_DEPLOYMENT_TARGET 11.0)
set(VCPKG_BUILD_TYPE release)

# OpenCascade: only the toolkits for STEP and STL and what they depend on, about half the build.
if(PORT STREQUAL "opencascade")
    set(VCPKG_CMAKE_CONFIGURE_OPTIONS
        -DBUILD_MODULE_ModelingAlgorithms=OFF
        -DBUILD_MODULE_Visualization=OFF
        -DBUILD_MODULE_ApplicationFramework=OFF
        -DBUILD_MODULE_DataExchange=OFF
        "-DBUILD_ADDITIONAL_TOOLKITS=TKDESTEP TKDESTL TKPrim TKMesh")
endif()
