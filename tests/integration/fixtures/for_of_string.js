// @expected-stdout: h,e,l,l,o
var chars = [];
for (var c of "hello") {
    chars.push(c);
}
console.log(chars.join(","));
