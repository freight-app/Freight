#include <nlohmann/json.hpp>  // github dep: nlohmann/json
#include <cxxopts.hpp>        // http dep:   jarro2783/cxxopts
#include <zlib.h>             // pkg_config: zlib

#include <cstring>
#include <iostream>
#include <string>
#include <vector>

int main(int argc, char** argv) {
    cxxopts::Options opts("with-external-deps",
        "crane example: github + http + pkg_config deps");
    opts.add_options()
        ("h,help",    "Print help")
        ("j,json",    "JSON object to parse",
            cxxopts::value<std::string>()
                ->default_value(R"({"lang":"C++","version":17})"))
        ("c,compress","Compress the serialised JSON with zlib and show ratio");

    auto args = opts.parse(argc, argv);
    if (args.count("help")) {
        std::cout << opts.help();
        return 0;
    }

    // ── Parse JSON (nlohmann/json, github dep) ────────────────────────────────
    std::string raw = args["json"].as<std::string>();
    nlohmann::json j;
    try {
        j = nlohmann::json::parse(raw);
    } catch (const nlohmann::json::parse_error& e) {
        std::cerr << "JSON parse error: " << e.what() << "\n";
        return 1;
    }
    std::cout << "Parsed JSON:\n" << j.dump(2) << "\n";

    // ── Optionally compress (zlib, pkg_config dep) ────────────────────────────
    if (args.count("compress")) {
        std::string src = j.dump();
        uLong bound = compressBound(static_cast<uLong>(src.size()));
        std::vector<Bytef> dst(bound);
        int rc = compress(dst.data(), &bound,
                          reinterpret_cast<const Bytef*>(src.data()),
                          static_cast<uLong>(src.size()));
        if (rc != Z_OK) {
            std::cerr << "zlib compress failed: " << rc << "\n";
            return 1;
        }
        std::cout << "Compressed " << src.size() << " → " << bound
                  << " bytes  (zlib " << zlibVersion() << ")\n";
    }

    return 0;
}
