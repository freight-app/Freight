#pragma once
#include <string>
#include <string_view>

std::string base64_encode(std::string_view input);
std::string base64_decode(std::string_view input);
