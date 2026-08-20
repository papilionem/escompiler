// @expected-stdout-begin
// 15
// 5hello
// 6
// 3
// @expected-stdout-end
let a = 10;
a += 5;
console.log(a);
let b = 5;
b += "hello";
console.log(b);
let c = 2;
c *= 3;
console.log(c);
let d = 10;
d -= 7;
console.log(d);
