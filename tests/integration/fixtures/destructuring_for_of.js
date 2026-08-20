// @expected-stdout: a:1 b:2 c:3
const entries = [["a", 1], ["b", 2], ["c", 3]];
let result = [];
for (const [key, val] of entries) {
  result.push(key + ":" + val);
}
console.log(result.join(" "));
