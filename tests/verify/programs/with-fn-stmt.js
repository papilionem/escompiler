var o = { a: 1 };
function outer() { with (o) { console.log("inside"); } console.log("done"); }
outer();
