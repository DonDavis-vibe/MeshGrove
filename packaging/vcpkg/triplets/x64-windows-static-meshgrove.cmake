# Windows release packages: static libraries and static C runtime (no Visual C++ Redistributable
# needed), release builds only. The name must contain "-static" for the vcpkg crate.
set(VCPKG_TARGET_ARCHITECTURE x64)
set(VCPKG_CRT_LINKAGE static)
set(VCPKG_LIBRARY_LINKAGE static)
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
