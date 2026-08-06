print("REI 0.1")
print("Personal AI Assistant")

name = input("What is your name? ")

print(f"Hello, {name}. REI is online.")

while True:
    command = input("You: ")

    if command == "hello":
        print(f"REI: Hello, {name}!")

    elif command == "status":
        print("REI: All systems operational.")

    elif command == "exit":
        print("REI: Going offline.")
        break

    else:
        print(f"REI: I don't understand '{command}' yet.")