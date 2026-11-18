#pragma once
#include "rust/cxx.h"
#include <memory>
namespace megawren {
class CacheCipher {
    struct Impl;
    std::unique_ptr<Impl> impl;
public:
    CacheCipher(rust::Slice<const uint8_t> key, rust::Slice<const uint8_t> header);
    ~CacheCipher();
    rust::Vec<uint8_t> header() const;
    void encrypt(rust::Slice<const uint8_t> bytes, rust::Slice<uint8_t> output, bool finalChunk);
    bool decrypt(rust::Slice<const uint8_t> bytes, rust::Slice<uint8_t> output);
};
rust::Vec<uint8_t> cache_key(rust::Slice<const uint8_t> session);
std::unique_ptr<CacheCipher> cache_cipher(rust::Slice<const uint8_t> key, rust::Slice<const uint8_t> header);
}
