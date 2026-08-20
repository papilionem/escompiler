// @expected-stdout-begin
// ok
// ok
// ok
// @expected-stdout-end

// NaN never matches
let matched = false;
switch(NaN) {
    case NaN: matched = true; break;
}
if (!matched) console.log("ok");

// -0 === +0
switch(0) {
    case -0: console.log("ok"); break;
    default: console.log("fail");
}

// String comparison in switch
switch("hello") {
    case "hello": console.log("ok"); break;
    default: console.log("fail");
}
