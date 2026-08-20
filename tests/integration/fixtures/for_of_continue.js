// @expected-stdout: 1,3,5
var odds = [];
for (var x of [1, 2, 3, 4, 5]) {
    if (x % 2 === 0) continue;
    odds.push(x);
}
console.log(odds.join(","));
