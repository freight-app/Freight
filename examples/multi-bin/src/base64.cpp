#include "base64.hpp"
#include <stdexcept>

static constexpr std::string_view TABLE =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

std::string base64_encode(std::string_view input) {
    std::string out;
    out.reserve((input.size() + 2) / 3 * 4);
    auto *data = reinterpret_cast<const unsigned char *>(input.data());
    size_t i = 0;
    for (; i + 2 < input.size(); i += 3) {
        out += TABLE[(data[i] >> 2) & 0x3F];
        out += TABLE[((data[i] & 0x3) << 4) | ((data[i+1] >> 4) & 0xF)];
        out += TABLE[((data[i+1] & 0xF) << 2) | ((data[i+2] >> 6) & 0x3)];
        out += TABLE[data[i+2] & 0x3F];
    }
    if (i < input.size()) {
        out += TABLE[(data[i] >> 2) & 0x3F];
        if (i + 1 < input.size()) {
            out += TABLE[((data[i] & 0x3) << 4) | ((data[i+1] >> 4) & 0xF)];
            out += TABLE[(data[i+1] & 0xF) << 2];
        } else {
            out += TABLE[(data[i] & 0x3) << 4];
            out += '=';
        }
        out += '=';
    }
    return out;
}

std::string base64_decode(std::string_view input) {
    static constexpr unsigned char DECODE[256] = {
        64,64,64,64,64,64,64,64,64,64,64,64,64,64,64,64,
        64,64,64,64,64,64,64,64,64,64,64,64,64,64,64,64,
        64,64,64,64,64,64,64,64,64,64,64,62,64,64,64,63,
        52,53,54,55,56,57,58,59,60,61,64,64,64, 0,64,64,
        64, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9,10,11,12,13,14,
        15,16,17,18,19,20,21,22,23,24,25,64,64,64,64,64,
        64,26,27,28,29,30,31,32,33,34,35,36,37,38,39,40,
        41,42,43,44,45,46,47,48,49,50,51,64,64,64,64,64,
    };

    std::string out;
    out.reserve(input.size() / 4 * 3);

    for (size_t i = 0; i < input.size(); i += 4) {
        unsigned char a = DECODE[(unsigned char)input[i]];
        unsigned char b = DECODE[(unsigned char)input[i+1]];
        unsigned char c = (i+2 < input.size()) ? DECODE[(unsigned char)input[i+2]] : 0u;
        unsigned char d = (i+3 < input.size()) ? DECODE[(unsigned char)input[i+3]] : 0u;
        if (a == 64 || b == 64) break;
        out += static_cast<char>((a << 2) | (b >> 4));
        if (input[i+2] != '=') out += static_cast<char>(((b & 0xF) << 4) | (c >> 2));
        if (input[i+3] != '=') out += static_cast<char>(((c & 0x3) << 6) | d);
    }
    return out;
}
