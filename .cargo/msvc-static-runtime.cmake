# Cargo's x86_64-pc-windows-msvc target uses Rust's `crt-static` feature, and
# every CMake-built native dependency must agree. A Visual Studio multi-config
# generator stores its runtime choice in the configuration-specific flags, so
# changing only CMAKE_C_FLAGS/CMAKE_CXX_FLAGS still leaves Release on /MD.
if(MSVC)
    foreach(language C CXX)
        foreach(configuration DEBUG RELEASE MINSIZEREL RELWITHDEBINFO)
            set(variable "CMAKE_${language}_FLAGS_${configuration}")
            set(flags "${${variable}}")
            string(REGEX REPLACE "[/-]M[DT]d?([ ]|$)" "" flags "${flags}")

            if(configuration STREQUAL "DEBUG")
                set(runtime "/MTd")
            else()
                set(runtime "/MT")
            endif()

            string(STRIP "${runtime} ${flags}" flags)
            set(${variable} "${flags}" CACHE STRING
                "Flags used by the ${language} compiler for ${configuration} builds"
                FORCE)
        endforeach()
    endforeach()
endif()
