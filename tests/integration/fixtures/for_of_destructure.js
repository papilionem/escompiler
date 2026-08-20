// @expected-stdout-begin
// 1 2
// 3 4
// @expected-stdout-end
for (const [a, b] of [[1, 2], [3, 4]]) {
    console.log(a, b);
}
