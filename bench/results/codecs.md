# Bundle encoders: size and speed

Each fixture's uncompressed bundle (`app.phext`) through every encoder. MB/s is of uncompressed bytes, single-threaded, median of repeated runs on this machine. "Rounds" is how many slow-start round trips (initcwnd 10) the encoded bundle needs; the default budget is 2.

## small (phext): 32892 bytes uncompressed

| encoder | bytes | vs gzip -9 | rounds | compress MB/s | decompress MB/s |
|---|---|---|---|---|---|
| gzip flate2 -1 | 15193 | +14.0% | 2 | 142.7 | 344 |
| gzip flate2 -6 | 13325 | +0.0% | 1 | 51.6 | 375 |
| gzip flate2 -9 | 13324 | +0.0% | 1 | 52.7 | 322 |
| gzip libdeflate -1 | 13779 | +3.4% | 1 | 225.0 | 736 |
| gzip libdeflate -6 | 12523 | -6.0% | 1 | 99.7 | 749 |
| gzip libdeflate -9 | 12470 | -6.4% | 1 | 60.9 | 749 |
| gzip libdeflate -12 | 11934 | -10.4% | 1 | 9.6 | 779 |
| gzip zopfli i15 | 11632 | -12.7% | 1 | 0.1 | 588 |
| br q1 | 15143 | +13.7% | 2 | 184.6 | 261 |
| br q5 | 12194 | -8.5% | 1 | 39.6 | 298 |
| br q9 | 12039 | -9.6% | 1 | 4.7 | 331 |
| br q11 | 11317 | -15.1% | 1 | 1.3 | 386 |
| zstd -1 | 13633 | +2.3% | 1 | 307.1 | 1200 |
| zstd -3 | 13113 | -1.6% | 1 | 406.1 | 1354 |
| zstd -9 | 12685 | -4.8% | 1 | 71.1 | 1412 |
| zstd -19 | 12264 | -8.0% | 1 | 6.6 | 1321 |
| zstd -22 | 12264 | -8.0% | 1 | 6.6 | 1359 |

## channels (phext): 76500 bytes uncompressed

| encoder | bytes | vs gzip -9 | rounds | compress MB/s | decompress MB/s |
|---|---|---|---|---|---|
| gzip flate2 -1 | 25751 | +32.8% | 2 | 245.7 | 427 |
| gzip flate2 -6 | 19457 | +0.4% | 2 | 36.1 | 557 |
| gzip flate2 -9 | 19384 | +0.0% | 2 | 24.1 | 548 |
| gzip libdeflate -1 | 21672 | +11.8% | 2 | 410.6 | 1590 |
| gzip libdeflate -6 | 19664 | +1.4% | 2 | 101.1 | 1759 |
| gzip libdeflate -9 | 19016 | -1.9% | 2 | 25.3 | 1796 |
| gzip libdeflate -12 | 17577 | -9.3% | 2 | 5.2 | 1875 |
| gzip zopfli i15 | 17387 | -10.3% | 2 | 0.1 | 153 |
| br q1 | 27571 | +42.2% | 2 | 99.7 | 111 |
| br q5 | 20059 | +3.5% | 2 | 15.7 | 147 |
| br q9 | 18237 | -5.9% | 2 | 4.3 | 285 |
| br q11 | 16591 | -14.4% | 2 | 0.4 | 113 |
| zstd -1 | 22125 | +14.1% | 2 | 167.0 | 781 |
| zstd -3 | 21075 | +8.7% | 2 | 219.2 | 827 |
| zstd -9 | 19547 | +0.8% | 2 | 14.2 | 357 |
| zstd -19 | 17168 | -11.4% | 2 | 2.1 | 600 |
| zstd -22 | 17164 | -11.5% | 2 | 1.6 | 258 |

## icons (phext): 51586 bytes uncompressed

| encoder | bytes | vs gzip -9 | rounds | compress MB/s | decompress MB/s |
|---|---|---|---|---|---|
| gzip flate2 -1 | 34800 | +2.1% | 2 | 162.0 | 344 |
| gzip flate2 -6 | 34097 | +0.0% | 2 | 36.1 | 332 |
| gzip flate2 -9 | 34097 | +0.0% | 2 | 24.4 | 133 |
| gzip libdeflate -1 | 33975 | -0.4% | 2 | 56.5 | 416 |
| gzip libdeflate -6 | 33088 | -3.0% | 2 | 78.8 | 554 |
| gzip libdeflate -9 | 33084 | -3.0% | 2 | 87.0 | 302 |
| gzip libdeflate -12 | 33213 | -2.6% | 2 | 5.5 | 560 |
| gzip zopfli i15 | 32789 | -3.8% | 2 | 0.1 | 250 |
| br q1 | 34692 | +1.7% | 2 | 105.7 | 131 |
| br q5 | 32926 | -3.4% | 2 | 32.8 | 187 |
| br q9 | 32867 | -3.6% | 2 | 2.4 | 168 |
| br q11 | 32452 | -4.8% | 2 | 0.5 | 142 |
| zstd -1 | 33739 | -1.0% | 2 | 349.5 | 1019 |
| zstd -3 | 33563 | -1.6% | 2 | 316.9 | 1013 |
| zstd -9 | 33403 | -2.0% | 2 | 89.4 | 1022 |
| zstd -19 | 33562 | -1.6% | 2 | 7.1 | 327 |
| zstd -22 | 33562 | -1.6% | 2 | 3.8 | 281 |

## spa (phext): 222752 bytes uncompressed

| encoder | bytes | vs gzip -9 | rounds | compress MB/s | decompress MB/s |
|---|---|---|---|---|---|
| gzip flate2 -1 | 75283 | +39.3% | 3 | 193.1 | 200 |
| gzip flate2 -6 | 54443 | +0.7% | 3 | 12.0 | 283 |
| gzip flate2 -9 | 54049 | +0.0% | 3 | 5.0 | 324 |
| gzip libdeflate -1 | 62081 | +14.9% | 3 | 118.3 | 640 |
| gzip libdeflate -6 | 56800 | +5.1% | 3 | 24.9 | 519 |
| gzip libdeflate -9 | 52446 | -3.0% | 3 | 12.0 | 437 |
| gzip libdeflate -12 | 48947 | -9.4% | 3 | 1.9 | 597 |
| gzip zopfli i15 | 48516 | -10.2% | 3 | 0.1 | 108 |
| br q1 | 81040 | +49.9% | 3 | 85.2 | 83 |
| br q5 | 58228 | +7.7% | 3 | 26.6 | 285 |
| br q9 | 49971 | -7.5% | 3 | 3.2 | 601 |
| br q11 | 45771 | -15.3% | 3 | 0.4 | 142 |
| zstd -1 | 63066 | +16.7% | 3 | 82.6 | 761 |
| zstd -3 | 59595 | +10.3% | 3 | 230.9 | 752 |
| zstd -9 | 56280 | +4.1% | 3 | 15.1 | 296 |
| zstd -19 | 46527 | -13.9% | 3 | 1.8 | 1002 |
| zstd -22 | 46527 | -13.9% | 3 | 2.2 | 359 |

## spa (phext-all): 394072 bytes uncompressed

| encoder | bytes | vs gzip -9 | rounds | compress MB/s | decompress MB/s |
|---|---|---|---|---|---|
| gzip flate2 -1 | 206205 | +11.6% | 4 | 63.4 | 97 |
| gzip flate2 -6 | 185279 | +0.3% | 4 | 12.2 | 168 |
| gzip flate2 -9 | 184794 | +0.0% | 4 | 7.6 | 117 |
| gzip libdeflate -1 | 192853 | +4.4% | 4 | 49.5 | 201 |
| gzip libdeflate -6 | 185421 | +0.3% | 4 | 55.4 | 725 |
| gzip libdeflate -9 | 180949 | -2.1% | 4 | 21.9 | 435 |
| gzip libdeflate -12 | 177003 | -4.2% | 4 | 3.4 | 242 |
| gzip zopfli i15 | 176724 | -4.4% | 4 | 0.1 | 235 |
| br q1 | 210574 | +14.0% | 4 | 42.6 | 50 |
| br q5 | 186866 | +1.1% | 4 | 19.3 | 210 |
| br q9 | 178620 | -3.3% | 4 | 4.1 | 74 |
| br q11 | 174147 | -5.8% | 4 | 0.6 | 246 |
| zstd -1 | 192979 | +4.4% | 4 | 346.4 | 874 |
| zstd -3 | 187212 | +1.3% | 4 | 262.2 | 952 |
| zstd -9 | 184603 | -0.1% | 4 | 45.6 | 898 |
| zstd -19 | 177157 | -4.1% | 4 | 2.8 | 444 |
| zstd -22 | 177157 | -4.1% | 4 | 3.5 | 509 |

