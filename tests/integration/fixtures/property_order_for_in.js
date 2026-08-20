// @expected-stdout: 0,1,2,a,b
var obj = {};
obj.a = 1;
obj[1] = 2;
obj.b = 3;
obj[0] = 4;
obj[2] = 5;
var keys = [];
for (var k in obj) {
  keys.push(k);
}
console.log(keys.join(","));
