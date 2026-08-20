// @expected-stdout: 0,1,10,b,c,2a
var obj = {};
obj.b = 1;
obj["0"] = 2;
obj.c = 3;
obj["10"] = 4;
obj["1"] = 5;
obj["2a"] = 6;
console.log(Object.keys(obj).join(","));
