// @expected-stdout-begin
// truthy
// truthy
// truthy
// @expected-stdout-end
if ({}) {
    console.log("truthy");
} else {
    console.log("falsy");
}
if ([]) {
    console.log("truthy");
} else {
    console.log("falsy");
}
if (function(){}) {
    console.log("truthy");
} else {
    console.log("falsy");
}
