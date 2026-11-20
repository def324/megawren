# Included after the SDK's project() call in the static Linux build.
# musl defines NULL as nullptr in C++. OpenSSL 3 expands its retired locking
# callbacks to (NULL), and GCC rejects negating nullptr in the SDK's check.
set_source_files_properties("${CMAKE_SOURCE_DIR}/src/posix/net.cpp"
    PROPERTIES COMPILE_OPTIONS -fpermissive)
