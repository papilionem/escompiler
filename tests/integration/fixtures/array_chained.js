// @expected-stdout: 30,40
console.log([1, 2, 3, 4].filter(x => x > 2).map(x => x * 10).join(","));
