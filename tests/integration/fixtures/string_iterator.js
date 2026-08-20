// @expected-stdout: a,b,c
let chars = [];
for (let c of "abc") chars.push(c);
console.log(chars.join(","));
