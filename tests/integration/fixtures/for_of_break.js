// @expected-stdout: 2
var count = 0;
for (var x of [1, 2, 3, 4, 5]) {
    if (x === 3) break;
    count++;
}
console.log(count);
