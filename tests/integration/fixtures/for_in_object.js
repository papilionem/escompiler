// @expected-stdout: ab
let keys = "";
for (const k in { a: 1, b: 2 }) {
    keys = keys + k;
}
console.log(keys);
