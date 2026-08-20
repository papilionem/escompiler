// @expected-stdout-begin
// falsy
// truthy
// falsy
// @expected-stdout-end
if ("") {
    console.log("truthy");
} else {
    console.log("falsy");
}
if ("hello") {
    console.log("truthy");
} else {
    console.log("falsy");
}
if ("" || false) {
    console.log("truthy");
} else {
    console.log("falsy");
}
