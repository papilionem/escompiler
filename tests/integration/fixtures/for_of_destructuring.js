// @expected-stdout-begin
// 1,2,3
// a,b,c
// @expected-stdout-end
var entries = [[1, "a"], [2, "b"], [3, "c"]];
var keys = [];
var vals = [];
for (var [k, v] of entries) {
    keys.push(k);
    vals.push(v);
}
console.log(keys.join(","));
console.log(vals.join(","));
