// @expected-stdout-begin
// initialized
// 42
// @expected-stdout-end

class Config {
    static value;
    static {
        Config.value = 42;
    }
    static label;
    static {
        Config.label = "initialized";
    }
}

console.log(Config.label);
console.log(Config.value);
