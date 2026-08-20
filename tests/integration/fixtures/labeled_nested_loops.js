// @expected-stdout: 6
// Nested labels: outer break and inner continue
let count = 0;
outer: for (let i = 0; i < 3; i++) {
    inner: for (let j = 0; j < 5; j++) {
        if (j === 2) continue inner;
        if (j === 3) continue outer;
        count = count + 1;
    }
}
// i=0: j=0 (+1), j=1 (+1), j=2 (skip), j=3 (continue outer) => 2
// i=1: j=0 (+1), j=1 (+1), j=2 (skip), j=3 (continue outer) => 2
// i=2: j=0 (+1), j=1 (+1), j=2 (skip), j=3 (continue outer) => 2
// total = 6
console.log(count);
