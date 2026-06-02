#include <fmt/core.h>    // registry dep: fmt  10.2.0
#include <zlib.h>        // registry dep: zlib 1.3.1

#include <string>
#include <vector>

static std::string compress_string(const std::string& src) {
    uLong bound = compressBound(static_cast<uLong>(src.size()));
    std::vector<Bytef> dst(bound);
    uLong dst_len = bound;
    if (compress(dst.data(), &dst_len,
                 reinterpret_cast<const Bytef*>(src.data()),
                 static_cast<uLong>(src.size())) != Z_OK) {
        return {};
    }
    return std::string(reinterpret_cast<char*>(dst.data()), dst_len);
}

int main() {
    // Registry metadata we'd find via `freight info`.
    struct Pkg { const char* name; const char* version; const char* desc; };
    constexpr Pkg packages[] = {
        { "fmt",  "10.2.0", "Formatting library for C++" },
        { "zlib", "1.3.1",  "A compression library"      },
    };

    fmt::print("Packages resolved from local registry:\n\n");
    for (const auto& p : packages) {
        fmt::print("  {:<16} {}  — {}\n", p.name, p.version, p.desc);
    }

    // Exercise zlib: compress a string and report the ratio.
    std::string payload = fmt::format(
        "freight registry example — {} packages, zlib {}", 2, zlibVersion());

    auto compressed = compress_string(payload);
    fmt::print("\nzlib {}: compressed {} → {} bytes ({:.0f}%)\n",
               zlibVersion(),
               payload.size(),
               compressed.size(),
               100.0 * compressed.size() / payload.size());

    return 0;
}
