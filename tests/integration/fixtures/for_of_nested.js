// @expected-stdout: 1-a,1-b,2-a,2-b
var results = [];
for (var x of [1, 2]) {
    for (var y of ["a", "b"]) {
        results.push(x + "-" + y);
    }
}
console.log(results.join(","));
