// @expected-stdout-begin
// 123
// 123
// @expected-stdout-end
let re = /(\d+)/;
let m = re.exec("abc123def");
console.log(m[0]);
console.log(m[1]);
