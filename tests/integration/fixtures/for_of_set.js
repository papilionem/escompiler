// @expected-stdout: 1,2,3,4
var s = new Set();
s.add(1);
s.add(2);
s.add(3);
s.add(4);
var result = [];
for (var x of s) {
    result.push(x);
}
console.log(result.join(","));
