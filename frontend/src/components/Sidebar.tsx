import { useState } from "react"

const buttons = [
    {
        "name": "Settings",
        "directory": ""
    },
    {
        "name": "Test",
        "directory": ""
    }
]

type ButtonProps = {
    name: String,
    directory: string
}
function ButtonComponent(props: ButtonProps) {
    return (
        <div className="sidebar-button">
            <button
            //  onClick={sendTo(props.directory)}
            >
                {props.name}
            </button>
        </div>
    )
}

export default function Sidebar() {
    const [fileList, setFileList] = useState([]);

    return (
        <div className="sidebar-container">
            {
                buttons.map(button => (
                    <ButtonComponent 
                     name={button.name}
                     directory={button.directory}/>
                ))    
            }
            <div className="eps-container">
                <p>eps-container here</p>
            </div>
        </div>
    )
}