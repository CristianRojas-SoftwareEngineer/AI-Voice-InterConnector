# Patch: escribe el contrato de enlace que lee `build.rs`. CMake lo inyecta con
# `CMAKE_PROJECT_INCLUDE` y, al diferir la escritura hasta el final de la
# configuración, las dos variables ya tienen su valor definitivo: la librería
# estándar de C++ que enlaza el compilador y el runtime OpenMP que se encontró.
# Así `build.rs` decide qué runtimes pide a partir de la configuración real y no
# de ficheros internos de CMake.
include_guard(GLOBAL)
function(_avi_write_link_contract)
  file(WRITE "${CMAKE_BINARY_DIR}/avi-link-runtimes.txt"
    "cxx=${CMAKE_CXX_IMPLICIT_LINK_LIBRARIES}\nopenmp=${OpenMP_CXX_LIB_NAMES}\n")
endfunction()
cmake_language(DEFER DIRECTORY "${CMAKE_SOURCE_DIR}" CALL _avi_write_link_contract)
